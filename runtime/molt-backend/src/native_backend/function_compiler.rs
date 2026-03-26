use super::*;

#[cfg(feature = "native-backend")]
static EMPTY_VEC_STRING: Vec<String> = Vec::new();

#[cfg(feature = "native-backend")]
struct FunctionPreanalysis {
    has_ret: bool,
    stateful: bool,
    has_store: bool,
    var_names: Vec<String>,
    last_use: BTreeMap<String, usize>,
    if_to_end_if: BTreeMap<usize, usize>,
    if_to_else: BTreeMap<usize, usize>,
    else_to_end_if: BTreeMap<usize, usize>,
    state_ids: Vec<i64>,
    resume_states: BTreeSet<i64>,
    function_exception_label_id: Option<i64>,
    /// Pre-built map from variable name -> constant integer value for O(1) lookups.
    /// Only the first definition of each name is stored (SSA correctness).
    const_int_map: BTreeMap<String, i64>,
}

#[cfg(feature = "native-backend")]
fn import_func_ref(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder,
    local_refs: &mut BTreeMap<&'static str, FuncRef>,
    name: &'static str,
    params: &[types::Type],
    returns: &[types::Type],
) -> FuncRef {
    static IMPORT_CACHE_DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let import_cache_disabled = *IMPORT_CACHE_DISABLED.get_or_init(|| {
        env_setting("MOLT_BACKEND_DISABLE_IMPORT_CACHE")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(false)
    });
    if let Some(func_ref) = local_refs.get(name) {
        return *func_ref;
    }
    let shape = ImportSignatureShape::from_types(params, returns);
    let func_id = if import_cache_disabled {
        let mut sig = module.make_signature();
        for param in params {
            sig.params.push(AbiParam::new(*param));
        }
        for ret in returns {
            sig.returns.push(AbiParam::new(*ret));
        }
        module
            .declare_function(name, Linkage::Import, &sig)
            .unwrap()
    } else {
        if let Some((func_id, cached_shape)) = import_ids.get(name) {
            assert_eq!(
                cached_shape, &shape,
                "import signature mismatch for {name}: {:?} vs {:?}",
                cached_shape, shape
            );
            *func_id
        } else {
            let mut sig = module.make_signature();
            for param in params {
                sig.params.push(AbiParam::new(*param));
            }
            for ret in returns {
                sig.returns.push(AbiParam::new(*ret));
            }
            let func_id = module
                .declare_function(name, Linkage::Import, &sig)
                .unwrap();
            import_ids.insert(name, (func_id, shape));
            func_id
        }
    };
    let func_ref = module.declare_func_in_func(func_id, builder.func);
    local_refs.insert(name, func_ref);
    func_ref
}

#[cfg(feature = "native-backend")]
fn preanalyze_function_ir(func_ir: &FunctionIR) -> FunctionPreanalysis {
    let mut has_ret = false;
    let mut stateful = false;
    let mut has_store = false;
    let mut var_names: BTreeSet<String> = BTreeSet::new();
    let mut last_use = BTreeMap::new();
    let mut if_to_end_if = BTreeMap::new();
    let mut if_to_else = BTreeMap::new();
    let mut else_to_end_if = BTreeMap::new();
    let mut if_stack: Vec<(usize, Option<usize>)> = Vec::new();
    let mut state_ids = Vec::new();
    let mut seen_state_ids: BTreeSet<i64> = BTreeSet::new();
    let mut resume_states = BTreeSet::new();
    let mut exception_label_ids = BTreeSet::new();
    let mut label_positions = Vec::new();

    for name in &func_ir.params {
        if name != "none" {
            var_names.insert(name.clone());
        }
    }

    for (idx, op) in func_ir.ops.iter().enumerate() {
        match op.kind.as_str() {
            "ret" | "ret_void" => has_ret = true,
            "state_switch" | "state_transition" | "state_yield" | "chan_send_yield"
            | "chan_recv_yield" => stateful = true,
            "store" => has_store = true,
            _ => {}
        }

        if let Some(out) = &op.out
            && out != "none"
        {
            var_names.insert(out.clone());
            if op.kind == "const_str" || op.kind == "const_bytes" {
                var_names.insert(format!("{}_ptr", out));
                var_names.insert(format!("{}_len", out));
            }
        }
        if let Some(var) = &op.var
            && var != "none"
        {
            var_names.insert(var.clone());
            last_use.insert(var.clone(), idx);
        }
        if let Some(args) = &op.args {
            for name in args {
                if name != "none" {
                    var_names.insert(name.clone());
                    last_use.insert(name.clone(), idx);
                }
            }
        }

        match op.kind.as_str() {
            "if" => if_stack.push((idx, None)),
            "else" => {
                if let Some((_, else_idx)) = if_stack.last_mut() {
                    *else_idx = Some(idx);
                }
            }
            "end_if" => {
                if let Some((if_idx, else_idx)) = if_stack.pop() {
                    if_to_end_if.insert(if_idx, idx);
                    if let Some(else_idx) = else_idx {
                        if_to_else.insert(if_idx, else_idx);
                        else_to_end_if.insert(else_idx, idx);
                    }
                }
            }
            "state_transition" | "state_yield" | "chan_send_yield" | "chan_recv_yield"
            | "label" | "state_label" => {
                if let Some(state_id) = op.value {
                    if seen_state_ids.insert(state_id) {
                        state_ids.push(state_id);
                    }
                    if op.kind == "state_yield" || op.kind == "state_label" {
                        resume_states.insert(state_id);
                    }
                    if matches!(op.kind.as_str(), "label" | "state_label") {
                        label_positions.push((idx, state_id));
                    }
                }
            }
            "check_exception" => {
                if let Some(label_id) = op.value {
                    exception_label_ids.insert(label_id);
                }
            }
            _ => {}
        }
    }

    // Post-pass: extend last_use for variables referenced inside loop bodies.
    // The linear scan above misses loop back-edges: a variable used only at
    // op N inside a loop body gets last_use = N, but if the loop iterates
    // again the variable is needed at op N again (which is reached via the
    // back-edge from loop_continue → loop_start).  Without this extension,
    // drain_cleanup_tracked at a check_exception site inside the loop body
    // can dec-ref the variable after the first iteration, freeing it before
    // the second iteration uses it.
    //
    // Fix: for every (loop_start..loop_end) range, extend last_use of all
    // variables referenced in that range to at least the loop_end index.
    //
    // Nested loops: ranges are collected as a flat list — an inner loop
    // (start_i, end_i) is always positionally contained within its outer
    // loop (start_o, end_o).  Variables used inside the inner loop appear
    // at positions within *both* ranges, so the max() logic naturally
    // extends their last_use to the outermost enclosing loop_end.  This is
    // conservative (inner-only variables survive until the outer loop_end)
    // but safe — premature free is the only correctness hazard here.
    //
    // While loops, break, continue: while loops emit loop_start/loop_end
    // (no loop_index_start), so they are covered.  loop_break/loop_continue
    // ops sit inside the range; variables they reference are extended.
    // At loop_break, drain_cleanup_tracked sees last_use > op_idx and
    // keeps variables alive; they propagate to after_block for later cleanup.
    {
        let mut loop_stack_post: Vec<usize> = Vec::new(); // stack of loop start indices
        let mut loop_ranges: Vec<(usize, usize)> = Vec::new();
        for (idx, op) in func_ir.ops.iter().enumerate() {
            match op.kind.as_str() {
                "loop_start" => {
                    // Only push if the next op is NOT loop_index_start.
                    // Indexed loops emit LOOP_START + LOOP_INDEX_START;
                    // the LOOP_INDEX_START is the real loop opener.
                    let indexed_follows = func_ir
                        .ops
                        .get(idx + 1)
                        .is_some_and(|next| next.kind == "loop_index_start");
                    if !indexed_follows {
                        loop_stack_post.push(idx);
                    }
                }
                "loop_index_start" => {
                    loop_stack_post.push(idx);
                }
                "loop_end" => {
                    if let Some(start) = loop_stack_post.pop() {
                        loop_ranges.push((start, idx));
                    }
                }
                _ => {}
            }
        }
        for (start, end) in &loop_ranges {
            for idx in *start..=*end {
                let op = &func_ir.ops[idx];
                if let Some(args) = &op.args {
                    for name in args {
                        if name != "none" {
                            let entry = last_use.entry(name.clone()).or_insert(*end);
                            if *entry < *end {
                                *entry = *end;
                            }
                        }
                    }
                }
                if let Some(var) = &op.var {
                    if var != "none" {
                        let entry = last_use.entry(var.clone()).or_insert(*end);
                        if *entry < *end {
                            *entry = *end;
                        }
                    }
                }
            }
        }
    }

    let mut var_names: Vec<String> = var_names.into_iter().collect();
    var_names.sort();
    let function_exception_label_id = label_positions
        .into_iter()
        .rev()
        .find_map(|(_, id)| exception_label_ids.contains(&id).then_some(id));

    let const_int_map = crate::build_const_int_map(&func_ir.ops);

    FunctionPreanalysis {
        has_ret,
        stateful,
        has_store,
        var_names,
        last_use,
        if_to_end_if,
        if_to_else,
        else_to_end_if,
        state_ids,
        resume_states,
        function_exception_label_id,
        const_int_map,
    }
}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    pub(crate) fn compile_func(
        &mut self,
        func_ir: FunctionIR,
        task_kinds: &BTreeMap<String, TrampolineKind>,
        task_closure_sizes: &BTreeMap<String, i64>,
        defined_functions: &BTreeSet<String>,
        closure_functions: &BTreeSet<String>,
        emit_traces: bool,
        leaf_functions: &BTreeSet<String>,
    ) {
        self.compile_func_inner(
            func_ir,
            task_kinds,
            task_closure_sizes,
            defined_functions,
            closure_functions,
            emit_traces,
            false,
            &BTreeSet::new(),
            leaf_functions,
        );
    }

    /// Inner compilation with optional `raw_int_mode` for typed-int twin
    /// functions.  When `raw_int_mode` is true, function parameters and
    /// return values use raw i64 instead of NaN-boxed representations,
    /// and all fast_int arithmetic ops skip boxing/unboxing.
    pub(crate) fn compile_func_inner(
        &mut self,
        func_ir: FunctionIR,
        task_kinds: &BTreeMap<String, TrampolineKind>,
        task_closure_sizes: &BTreeMap<String, i64>,
        defined_functions: &BTreeSet<String>,
        closure_functions: &BTreeSet<String>,
        emit_traces: bool,
        _raw_int_mode: bool,
        _typed_int_functions: &BTreeSet<String>,
        leaf_functions: &BTreeSet<String>,
    ) {
        let mut builder_ctx = FunctionBuilderContext::new();
        self.module.clear_context(&mut self.ctx);
        let FunctionPreanalysis {
            has_ret,
            stateful,
            has_store,
            var_names,
            last_use,
            if_to_end_if,
            if_to_else,
            else_to_end_if,
            state_ids,
            resume_states,
            function_exception_label_id,
            const_int_map: _const_int_map,
        } = preanalyze_function_ir(&func_ir);
        let (rc_skip_inc, rc_skip_dec) = crate::passes::compute_rc_coalesce_skips(&func_ir.ops, &last_use);

        if has_ret {
            self.ctx
                .func
                .signature
                .returns
                .push(AbiParam::new(types::I64));
        }
        for _ in &func_ir.params {
            self.ctx
                .func
                .signature
                .params
                .push(AbiParam::new(types::I64));
        }

        let param_types: Vec<types::Type> = self
            .ctx
            .func
            .signature
            .params
            .iter()
            .map(|p| p.value_type)
            .collect();
        let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut builder_ctx);

        let mut vars: BTreeMap<String, Variable> = BTreeMap::new();
        let param_name_set: BTreeSet<&str> = func_ir.params.iter().map(String::as_str).collect();
        for name in var_names.iter() {
            let var = builder.declare_var(types::I64);
            vars.insert(name.clone(), var);
        }
        let trace_ops = should_trace_ops(&func_ir.name);
        let trace_stride = trace_ops.as_ref().map(|cfg| cfg.stride);
        let mut trace_name_var: Option<Variable> = None;
        let mut trace_len_var: Option<Variable> = None;
        let mut trace_func: Option<FuncRef> = None;
        // When op tracing is enabled, we install the trace data segment and trace function ref
        // early, but we must not emit any instructions into the entry block until all block
        // parameters have been appended (Cranelift panics otherwise). We therefore defer the
        // `symbol_value` + `iconst` instructions until after parameter block params are created.
        let mut trace_data: Option<(cranelift_module::DataId, i64)> = None;
        let mut tracked_vars = Vec::new();
        let mut tracked_obj_vars = Vec::new();
        let mut tracked_vars_set: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut tracked_obj_vars_set: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut entry_vars: BTreeMap<String, Value> = BTreeMap::new();
        let mut state_blocks = BTreeMap::new();
        let mut import_refs: BTreeMap<&'static str, FuncRef> = BTreeMap::new();
        let mut reachable_blocks: BTreeSet<Block> = BTreeSet::new();
        // Cranelift SSA-variable correctness relies on sealing blocks once all predecessors
        // are known. Our IR uses structured control-flow; for `if` this means then/else
        // each have a single predecessor and can be sealed immediately, and the merge block
        // can be sealed once end_if wiring is complete.
        let mut sealed_blocks: BTreeSet<Block> = BTreeSet::new();
        let mut is_block_filled = false;
        let mut if_stack: Vec<IfFrame> = Vec::new();
        let mut loop_stack: Vec<LoopFrame> = Vec::new();
        // Map closure function names to their function object variable names
        let mut local_closure_envs: BTreeMap<String, String> = BTreeMap::new();
        let mut loop_depth: i32 = 0;
        let mut block_tracked_obj: BTreeMap<Block, Vec<String>> = BTreeMap::new();
        let mut block_tracked_ptr: BTreeMap<Block, Vec<String>> = BTreeMap::new();

        let entry_block = builder.create_block();
        let master_return_block = builder.create_block();
        if has_ret {
            builder.append_block_param(master_return_block, types::I64);
        }

        reachable_blocks.insert(entry_block);
        builder.switch_to_block(entry_block);

        let _local_dec_ref = import_func_ref(
            &mut self.module,
            &mut self.import_ids,
            &mut builder,
            &mut import_refs,
            "molt_dec_ref",
            &[types::I64],
            &[],
        );
        let local_dec_ref_obj = import_func_ref(
            &mut self.module,
            &mut self.import_ids,
            &mut builder,
            &mut import_refs,
            "molt_dec_ref_obj",
            &[types::I64],
            &[],
        );
        let local_inc_ref_obj = import_func_ref(
            &mut self.module,
            &mut self.import_ids,
            &mut builder,
            &mut import_refs,
            "molt_inc_ref_obj",
            &[types::I64],
            &[],
        );

        // Import the exception-pending function for check_exception.
        // The inline flag load optimization is applied lazily at the
        // first check_exception site to avoid Cranelift block ordering
        // issues with the entry block.
        let local_exc_pending_fast = import_func_ref(
            &mut self.module,
            &mut self.import_ids,
            &mut builder,
            &mut import_refs,
            "molt_exception_pending_fast",
            &[],
            &[types::I64],
        );
        // Inline exception flag optimization: fetch the flag pointer once
        // per block and inline a byte load at each check_exception site.
        // Fetch the exception flag pointer once in the entry block via a
        // Cranelift Variable (SSA propagates it automatically across all
        // blocks, including stateful/poll functions).  The Variable-based
        // approach uses declare_var/def_var/use_var which handles dominator
        // propagation through Switch-generated intermediate blocks correctly.
        let has_exc_handling = function_exception_label_id.is_some();
        static INLINE_EXC_DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let inline_exc_disabled = *INLINE_EXC_DISABLED.get_or_init(|| {
            env_setting("MOLT_BACKEND_INLINE_EXC_DISABLED")
                .as_deref()
                .map(parse_truthy_env)
                .unwrap_or(false)
        });
        let exc_flag_ptr_var: Option<Variable> = if has_exc_handling && !inline_exc_disabled {
            let var = builder.declare_var(types::I64);
            Some(var)
        } else {
            None
        };
        let exc_flag_ptr_fn = if has_exc_handling && !inline_exc_disabled {
            Some(import_func_ref(
                &mut self.module,
                &mut self.import_ids,
                &mut builder,
                &mut import_refs,
                "molt_exception_pending_flag_ptr",
                &[],
                &[types::I64],
            ))
        } else {
            None
        };
        // Per-block cache for the flag pointer Value (stateful functions only).
        let mut exc_flag_ptr_block_cache: BTreeMap<Block, Value> = BTreeMap::new();
        let local_profile_struct = has_store.then(|| {
            import_func_ref(
                &mut self.module,
                &mut self.import_ids,
                &mut builder,
                &mut import_refs,
                "molt_profile_struct_field_store",
                &[],
                &[],
            )
        });
        let local_profile_enabled = has_store.then(|| {
            import_func_ref(
                &mut self.module,
                &mut self.import_ids,
                &mut builder,
                &mut import_refs,
                "molt_profile_enabled",
                &[],
                &[types::I64],
            )
        });

        if trace_stride.is_some() {
            let trace_suffix: String = func_ir
                .name
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || ch == '_' {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect();
            let data_id = self
                .module
                .declare_data(
                    &format!("trace_fn_{trace_suffix}"),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(func_ir.name.as_bytes().to_vec().into_boxed_slice());
            self.module.define_data(data_id, &data_ctx).unwrap();
            trace_data = Some((data_id, func_ir.name.len() as i64));

            trace_func = Some(import_func_ref(
                &mut self.module,
                &mut self.import_ids,
                &mut builder,
                &mut import_refs,
                "molt_debug_trace",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            ));
        }

        for (i, ty) in param_types.iter().enumerate() {
            let val = builder.append_block_param(entry_block, *ty);

            let name = &func_ir.params[i];

            def_var_named(&mut builder, &vars, name, val);
        }

        let nbc = NanBoxConsts::new(&mut builder);

        if let Some((data_id, name_len_i64)) = trace_data {
            let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
            let name_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let name_len = builder.ins().iconst(types::I64, name_len_i64);

            let name_var = builder.declare_var(types::I64);
            builder.def_var(name_var, name_ptr);
            trace_name_var = Some(name_var);

            let len_var = builder.declare_var(types::I64);
            builder.def_var(len_var, name_len);
            trace_len_var = Some(len_var);
        }

        if stateful && vars.contains_key("self") {
            let self_ptr = var_get(&mut builder, &vars, "self").expect("Self not found");
            let self_bits = box_ptr_value(&mut builder, *self_ptr, &nbc);
            def_var_named(&mut builder, &vars, "self", self_bits);
        }

        let profile_enabled_val = local_profile_enabled.map(|local_profile_enabled| {
            let call = builder.ins().call(local_profile_enabled, &[]);
            builder.inst_results(call)[0]
        });

        // Fetch the exception flag pointer in the entry block and store it
        // in a Cranelift Variable.  The SSA system propagates the definition
        // across all blocks automatically (including stateful/poll functions).
        if let (Some(var), Some(fn_ref)) = (exc_flag_ptr_var, exc_flag_ptr_fn) {
            let call = builder.ins().call(fn_ref, &[]);
            let ptr_val = builder.inst_results(call)[0];
            builder.def_var(var, ptr_val);
        }

        builder.seal_block(entry_block);
        sealed_blocks.insert(entry_block);

        for state_id in state_ids {
            state_blocks
                .entry(state_id)
                .or_insert_with(|| builder.create_block());
        }

        // 2. Implementation
        let ops = &func_ir.ops;
        let mut skip_ops: BTreeSet<usize> = BTreeSet::new();
        // Stack-eligible tuples: when escape analysis marks a tuple_new as
        // stack_eligible, we keep its element Values in this map instead of
        // emitting runtime list-builder calls.  Index ops that reference a
        // stack tuple resolve the element at compile time.
        let mut stack_tuples: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for op_idx in 0..ops.len() {
            if skip_ops.contains(&op_idx) {
                continue;
            }
            let op = ops[op_idx].clone();
            sync_block_filled(&builder, &mut is_block_filled);
            if is_block_filled {
                if op.kind == "if"
                    && let Some(&end_if_idx) = if_to_end_if.get(&op_idx)
                {
                    for idx in op_idx..=end_if_idx {
                        skip_ops.insert(idx);
                    }
                    let mut phi_idx = end_if_idx + 1;
                    while phi_idx < ops.len() {
                        if ops[phi_idx].kind != "phi" {
                            break;
                        }
                        skip_ops.insert(phi_idx);
                        phi_idx += 1;
                    }
                    continue;
                }
                match op.kind.as_str() {
                    "label" | "state_label" | "else" | "end_if" | "loop_start"
                    | "loop_index_start" | "loop_index_next" | "loop_continue"
                    | "loop_end" | "loop_break" | "loop_break_if_true"
                    | "loop_break_if_false" => {}
                    _ => continue,
                }
            }
            if !is_block_filled
                && let Some(stride) = trace_stride
                && op_idx % stride == 0
                && let (Some(name_var), Some(len_var), Some(trace_fn)) =
                    (trace_name_var, trace_len_var, trace_func)
            {
                let name_bits = builder.use_var(name_var);
                let len_bits = builder.use_var(len_var);
                let idx_bits = builder.ins().iconst(types::I64, op_idx as i64);
                builder
                    .ins()
                    .call(trace_fn, &[name_bits, len_bits, idx_bits]);
            }
            let out_name = op.out.clone();
            let mut output_is_ptr = false;

            match op.kind.as_str() {
                "const" => {
                    let val = op.value.unwrap_or(0);
                    const INLINE_MIN: i64 = -(1_i64 << 46);
                    const INLINE_MAX: i64 = (1_i64 << 46) - 1;
                    if val >= INLINE_MIN && val <= INLINE_MAX {
                        let boxed = box_int(val);
                        let iconst = builder.ins().iconst(types::I64, boxed);
                        if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, iconst); }
                    } else {
                        // Value exceeds 47-bit signed inline range — use bigint path.
                        let s = val.to_string();
                        let bytes = s.as_bytes();
                        let data_id = Self::intern_data_segment(
                            &mut self.module,
                            &mut self.data_pool,
                            &mut self.next_data_id,
                            bytes,
                        );
                        let Some(out_name) = op.out else { continue; };
                        let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                        let ptr = builder.ins().symbol_value(types::I64, global_ptr);
                        let len = builder.ins().iconst(types::I64, bytes.len() as i64);
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bigint_from_str", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[ptr, len]);
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, out_name, res);
                        output_is_ptr = true;
                    }
                }
                "const_bigint" => {
                    let s = op.s_value.as_ref().expect("BigInt string not found");
                    let Some(out_name) = op.out else { continue; };
                    let bytes = s.as_bytes();
                    let data_id = Self::intern_data_segment(
                        &mut self.module,
                        &mut self.data_pool,
                        &mut self.next_data_id,
                        bytes,
                    );
                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let len = builder.ins().iconst(types::I64, bytes.len() as i64);

                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bigint_from_str", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[ptr, len]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, out_name, res);
                    output_is_ptr = true;
                }
                "const_bool" => {
                    let val = op.value.unwrap_or(0);
                    let boxed = box_bool(val);
                    let iconst = builder.ins().iconst(types::I64, boxed);
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, iconst); }
                }
                "const_none" => {
                    let iconst = builder.ins().iconst(types::I64, box_none());
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, iconst); }
                }
                "const_not_implemented" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_not_implemented", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "const_ellipsis" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_ellipsis", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "const_float" => {
                    let val = op.f_value.expect("Float value not found");
                    let boxed = box_float(val);
                    let iconst = builder.ins().iconst(types::I64, boxed);
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, iconst); }
                }
                "const_str" => {
                    let bytes = op
                        .bytes
                        .as_deref()
                        .unwrap_or_else(|| op.s_value.as_deref().unwrap_or("").as_bytes());
                    let Some(out_name) = op.out else { continue; };
                    let data_id = Self::intern_data_segment(
                        &mut self.module,
                        &mut self.data_pool,
                        &mut self.next_data_id,
                        bytes,
                    );

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let len = builder.ins().iconst(types::I64, bytes.len() as i64);

                    def_var_named(&mut builder, &vars, format!("{}_ptr", out_name), ptr);
                    def_var_named(&mut builder, &vars, format!("{}_len", out_name), len);

                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_from_bytes", &[types::I64, types::I64, types::I64], &[types::I32]);
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[ptr, len, out_ptr]);
                    let boxed = builder
                        .ins()
                        .load(types::I64, MemFlags::trusted(), out_ptr, 0);

                    def_var_named(&mut builder, &vars, out_name, boxed);
                }
                "const_bytes" => {
                    let bytes = op.bytes.as_ref().expect("Bytes not found");
                    let Some(out_name) = op.out else { continue; };
                    let data_id = Self::intern_data_segment(
                        &mut self.module,
                        &mut self.data_pool,
                        &mut self.next_data_id,
                        bytes,
                    );

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let len = builder.ins().iconst(types::I64, bytes.len() as i64);

                    def_var_named(&mut builder, &vars, format!("{}_ptr", out_name), ptr);
                    def_var_named(&mut builder, &vars, format!("{}_len", out_name), len);

                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_from_bytes", &[types::I64, types::I64, types::I64], &[types::I32]);
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[ptr, len, out_ptr]);
                    let boxed = builder
                        .ins()
                        .load(types::I64, MemFlags::trusted(), out_ptr, 0);

                    def_var_named(&mut builder, &vars, out_name, boxed);
                }
                "add" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false)
                        || op.type_hint.as_deref() == Some("float")
                    {
                        // Both operands known to be f64 — direct float arithmetic.
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fadd(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f, &nbc)
                    } else if op.fast_int.unwrap_or(false)
                        || op.type_hint.as_deref() == Some("int")
                    {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_add", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        // Guard: both operands must be inline ints (not bigint pointers).
                        // fast_int assumes NaN-boxed inline ints, but bigints are heap-
                        // allocated pointers with a different tag. Unboxing a pointer as
                        // an inline int produces garbage.
                        let lhs_is_int = is_int_or_bool_tag(&mut builder, *lhs, &nbc);
                        let rhs_is_int = is_int_or_bool_tag(&mut builder, *rhs, &nbc);
                        let both_inline = builder.ins().band(lhs_is_int, rhs_is_int);
                        let guard_fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder.ins().brif(both_inline, guard_fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(guard_fast_block);
                        builder.seal_block(guard_fast_block);
                        let fast_block = builder.create_block();
                        // Use unbox_int_or_bool: Python booleans are ints (True+True==2).
                        let lhs_val = unbox_int_or_bool(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int_or_bool(&mut builder, *rhs, &nbc);
                        let sum = builder.ins().iadd(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, sum, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, sum);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_add", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let sum = builder.ins().iadd(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, sum, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, sum);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        // Inline float fast path: if both operands are floats, do f64 add directly.
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs, &nbc);
                        let float_block = builder.create_block();
                        let call_block = builder.create_block();
                        builder.set_cold_block(call_block);
                        builder
                            .ins()
                            .brif(both_flt, float_block, &[], call_block, &[]);

                        builder.switch_to_block(float_block);
                        builder.seal_block(float_block);
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let flt_sum = builder.ins().fadd(lhs_f, rhs_f);
                        let flt_res = box_float_value(&mut builder, flt_sum, &nbc);
                        jump_block(&mut builder, merge_block, &[flt_res]);

                        builder.switch_to_block(call_block);
                        builder.seal_block(call_block);
                        emit_mixed_int_float_op(
                            &mut builder, *lhs, *rhs, &nbc, 0, merge_block,
                        );
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "inplace_add" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false)
                        || op.type_hint.as_deref() == Some("float")
                    {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fadd(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f, &nbc)
                    } else if op.fast_int.unwrap_or(false)
                        || op.type_hint.as_deref() == Some("int")
                    {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_inplace_add", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        // Use unbox_int_or_bool: Python booleans are ints.
                        let lhs_val = unbox_int_or_bool(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int_or_bool(&mut builder, *rhs, &nbc);
                        let sum = builder.ins().iadd(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, sum, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, sum);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_inplace_add", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let sum = builder.ins().iadd(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, sum, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, sum);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs, &nbc);
                        let float_block = builder.create_block();
                        let call_block = builder.create_block();
                        builder.set_cold_block(call_block);
                        builder
                            .ins()
                            .brif(both_flt, float_block, &[], call_block, &[]);

                        builder.switch_to_block(float_block);
                        builder.seal_block(float_block);
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let flt_sum = builder.ins().fadd(lhs_f, rhs_f);
                        let flt_res = box_float_value(&mut builder, flt_sum, &nbc);
                        jump_block(&mut builder, merge_block, &[flt_res]);

                        builder.switch_to_block(call_block);
                        builder.seal_block(call_block);
                        emit_mixed_int_float_op(
                            &mut builder, *lhs, *rhs, &nbc, 0, merge_block,
                        );
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_sum_int" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_sum_int", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_sum_int_trusted" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_sum_int_trusted", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_sum_int_range" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_sum_int_range", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_sum_int_range_trusted" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_sum_int_range_trusted", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_sum_int_range_iter" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_sum_int_range_iter", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_sum_int_range_iter_trusted" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_sum_int_range_iter_trusted", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_sum_float" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_sum_float", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_sum_float_trusted" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_sum_float_trusted", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_sum_float_range" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_sum_float_range", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_sum_float_range_trusted" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_sum_float_range_trusted", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_sum_float_range_iter" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_sum_float_range_iter", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_sum_float_range_iter_trusted" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_sum_float_range_iter_trusted", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_prod_int" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_prod_int", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_prod_int_trusted" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_prod_int_trusted", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_prod_int_range" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_prod_int_range", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_prod_int_range_trusted" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_prod_int_range_trusted", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_min_int" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_min_int", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_min_int_trusted" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_min_int_trusted", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_min_int_range" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_min_int_range", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_min_int_range_trusted" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_min_int_range_trusted", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_max_int" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_max_int", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_max_int_trusted" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_max_int_trusted", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_max_int_range" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_max_int_range", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "vec_max_int_range_trusted" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_vec_max_int_range_trusted", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "sub" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("LHS not found in {} op {}", func_ir.name, op_idx)
                    });
                    let rhs = var_get(&mut builder, &vars, &args[1]).unwrap_or_else(|| {
                        panic!("RHS not found in {} op {}", func_ir.name, op_idx)
                    });
                    let res = if op.fast_float.unwrap_or(false)
                        || op.type_hint.as_deref() == Some("float")
                    {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fsub(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f, &nbc)
                    } else if op.fast_int.unwrap_or(false)
                        || op.type_hint.as_deref() == Some("int")
                    {
                        // Inline isub with overflow check + BigInt fallback.
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_sub", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let diff = builder.ins().isub(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, diff, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, diff);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);
                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        jump_block(&mut builder, merge_block, &[fast_res]);
                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);
                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let diff = builder.ins().isub(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, diff, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, diff);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_sub", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs, &nbc);
                        let float_block = builder.create_block();
                        let call_block = builder.create_block();
                        builder.set_cold_block(call_block);
                        builder
                            .ins()
                            .brif(both_flt, float_block, &[], call_block, &[]);

                        builder.switch_to_block(float_block);
                        builder.seal_block(float_block);
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let flt_diff = builder.ins().fsub(lhs_f, rhs_f);
                        let flt_res = box_float_value(&mut builder, flt_diff, &nbc);
                        jump_block(&mut builder, merge_block, &[flt_res]);

                        builder.switch_to_block(call_block);
                        builder.seal_block(call_block);
                        emit_mixed_int_float_op(
                            &mut builder, *lhs, *rhs, &nbc, 1, merge_block,
                        );
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "inplace_sub" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("LHS not found in {} op {}", func_ir.name, op_idx)
                    });
                    let rhs = var_get(&mut builder, &vars, &args[1]).unwrap_or_else(|| {
                        panic!("RHS not found in {} op {}", func_ir.name, op_idx)
                    });
                    let res = if op.fast_float.unwrap_or(false)
                        || op.type_hint.as_deref() == Some("float")
                    {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fsub(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f, &nbc)
                    } else if op.fast_int.unwrap_or(false)
                        || op.type_hint.as_deref() == Some("int")
                    {
                        // Inline isub with overflow check + BigInt fallback.
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_sub", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let diff = builder.ins().isub(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, diff, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, diff);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);
                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        jump_block(&mut builder, merge_block, &[fast_res]);
                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);
                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let diff = builder.ins().isub(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, diff, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, diff);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_inplace_sub", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs, &nbc);
                        let float_block = builder.create_block();
                        let call_block = builder.create_block();
                        builder.set_cold_block(call_block);
                        builder
                            .ins()
                            .brif(both_flt, float_block, &[], call_block, &[]);

                        builder.switch_to_block(float_block);
                        builder.seal_block(float_block);
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let flt_diff = builder.ins().fsub(lhs_f, rhs_f);
                        let flt_res = box_float_value(&mut builder, flt_diff, &nbc);
                        jump_block(&mut builder, merge_block, &[flt_res]);

                        builder.switch_to_block(call_block);
                        builder.seal_block(call_block);
                        emit_mixed_int_float_op(
                            &mut builder, *lhs, *rhs, &nbc, 1, merge_block,
                        );
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "mul" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false)
                        || op.type_hint.as_deref() == Some("float")
                    {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fmul(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f, &nbc)
                    } else if op.fast_int.unwrap_or(false)
                        || op.type_hint.as_deref() == Some("int")
                    {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_mul", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let (prod, fits) = imul_checked_inline(&mut builder, lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, prod, &nbc);
                        builder
                            .ins()
                            .brif(fits, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_mul", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let (prod, fits) = imul_checked_inline(&mut builder, lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, prod, &nbc);
                        brif_block(
                            &mut builder,
                            fits,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs, &nbc);
                        let float_block = builder.create_block();
                        let call_block = builder.create_block();
                        builder.set_cold_block(call_block);
                        builder
                            .ins()
                            .brif(both_flt, float_block, &[], call_block, &[]);

                        builder.switch_to_block(float_block);
                        builder.seal_block(float_block);
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let flt_prod = builder.ins().fmul(lhs_f, rhs_f);
                        let flt_res = box_float_value(&mut builder, flt_prod, &nbc);
                        jump_block(&mut builder, merge_block, &[flt_res]);

                        builder.switch_to_block(call_block);
                        builder.seal_block(call_block);
                        emit_mixed_int_float_op(
                            &mut builder, *lhs, *rhs, &nbc, 2, merge_block,
                        );
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "inplace_mul" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false)
                        || op.type_hint.as_deref() == Some("float")
                    {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fmul(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f, &nbc)
                    } else if op.fast_int.unwrap_or(false)
                        || op.type_hint.as_deref() == Some("int")
                    {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_inplace_mul", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let (prod, fits) = imul_checked_inline(&mut builder, lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, prod, &nbc);
                        builder
                            .ins()
                            .brif(fits, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_inplace_mul", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let (prod, fits) = imul_checked_inline(&mut builder, lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, prod, &nbc);
                        brif_block(
                            &mut builder,
                            fits,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs, &nbc);
                        let float_block = builder.create_block();
                        let call_block = builder.create_block();
                        builder.set_cold_block(call_block);
                        builder
                            .ins()
                            .brif(both_flt, float_block, &[], call_block, &[]);

                        builder.switch_to_block(float_block);
                        builder.seal_block(float_block);
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let flt_prod = builder.ins().fmul(lhs_f, rhs_f);
                        let flt_res = box_float_value(&mut builder, flt_prod, &nbc);
                        jump_block(&mut builder, merge_block, &[flt_res]);

                        builder.switch_to_block(call_block);
                        builder.seal_block(call_block);
                        emit_mixed_int_float_op(
                            &mut builder, *lhs, *rhs, &nbc, 2, merge_block,
                        );
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bit_or" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bit_or", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let raw = builder.ins().bor(lhs_val, rhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, raw, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bit_or", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let raw = builder.ins().bor(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, raw, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "inplace_bit_or" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_inplace_bit_or", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let raw = builder.ins().bor(lhs_val, rhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, raw, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_inplace_bit_or", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let raw = builder.ins().bor(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, raw, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bit_and" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bit_and", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let raw = builder.ins().band(lhs_val, rhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, raw, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bit_and", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let raw = builder.ins().band(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, raw, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "inplace_bit_and" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_inplace_bit_and", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let raw = builder.ins().band(lhs_val, rhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, raw, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_inplace_bit_and", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let raw = builder.ins().band(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, raw, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bit_xor" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bit_xor", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let raw = builder.ins().bxor(lhs_val, rhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, raw, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bit_xor", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let raw = builder.ins().bxor(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, raw, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "inplace_bit_xor" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_inplace_bit_xor", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let raw = builder.ins().bxor(lhs_val, rhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, raw, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_inplace_bit_xor", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let raw = builder.ins().bxor(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, raw, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "lshift" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_lshift", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let range_block = builder.create_block();
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let max_shift = builder.ins().iconst(types::I64, 64);
                        let rhs_non_negative =
                            builder
                                .ins()
                                .icmp(IntCC::SignedGreaterThanOrEqual, rhs_val, zero);
                        let rhs_lt_limit =
                            builder
                                .ins()
                                .icmp(IntCC::SignedLessThan, rhs_val, max_shift);
                        let rhs_in_range = builder.ins().band(rhs_non_negative, rhs_lt_limit);
                        builder
                            .ins()
                            .brif(rhs_in_range, range_block, &[], slow_block, &[]);

                        builder.switch_to_block(range_block);
                        builder.seal_block(range_block);
                        let shifted = builder.ins().ishl(lhs_val, rhs_val);
                        let reversed = builder.ins().sshr(shifted, rhs_val);
                        let no_overflow = builder.ins().icmp(IntCC::Equal, reversed, lhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, shifted);
                        let can_inline = builder.ins().band(no_overflow, fits_inline);
                        builder
                            .ins()
                            .brif(can_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let shifted = builder.ins().ishl(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, shifted, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_lshift", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let int_block = builder.create_block();
                        let range_block = builder.create_block();
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        builder
                            .ins()
                            .brif(both_int, int_block, &[], slow_block, &[]);

                        builder.switch_to_block(int_block);
                        builder.seal_block(int_block);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let max_shift = builder.ins().iconst(types::I64, 64);
                        let rhs_non_negative =
                            builder
                                .ins()
                                .icmp(IntCC::SignedGreaterThanOrEqual, rhs_val, zero);
                        let rhs_lt_limit =
                            builder
                                .ins()
                                .icmp(IntCC::SignedLessThan, rhs_val, max_shift);
                        let rhs_in_range = builder.ins().band(rhs_non_negative, rhs_lt_limit);
                        builder
                            .ins()
                            .brif(rhs_in_range, range_block, &[], slow_block, &[]);

                        builder.switch_to_block(range_block);
                        builder.seal_block(range_block);
                        let shifted = builder.ins().ishl(lhs_val, rhs_val);
                        let reversed = builder.ins().sshr(shifted, rhs_val);
                        let no_overflow = builder.ins().icmp(IntCC::Equal, reversed, lhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, shifted);
                        let can_inline = builder.ins().band(no_overflow, fits_inline);
                        builder
                            .ins()
                            .brif(can_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let shifted = builder.ins().ishl(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, shifted, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "rshift" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_rshift", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let max_shift = builder.ins().iconst(types::I64, 64);
                        let rhs_non_negative =
                            builder
                                .ins()
                                .icmp(IntCC::SignedGreaterThanOrEqual, rhs_val, zero);
                        let rhs_lt_limit =
                            builder
                                .ins()
                                .icmp(IntCC::SignedLessThan, rhs_val, max_shift);
                        let rhs_in_range = builder.ins().band(rhs_non_negative, rhs_lt_limit);
                        builder
                            .ins()
                            .brif(rhs_in_range, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let shifted = builder.ins().sshr(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, shifted, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_rshift", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let int_block = builder.create_block();
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        builder
                            .ins()
                            .brif(both_int, int_block, &[], slow_block, &[]);

                        builder.switch_to_block(int_block);
                        builder.seal_block(int_block);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let max_shift = builder.ins().iconst(types::I64, 64);
                        let rhs_non_negative =
                            builder
                                .ins()
                                .icmp(IntCC::SignedGreaterThanOrEqual, rhs_val, zero);
                        let rhs_lt_limit =
                            builder
                                .ins()
                                .icmp(IntCC::SignedLessThan, rhs_val, max_shift);
                        let rhs_in_range = builder.ins().band(rhs_non_negative, rhs_lt_limit);
                        builder
                            .ins()
                            .brif(rhs_in_range, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let shifted = builder.ins().sshr(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, shifted, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "matmul" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_matmul", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "div" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        // Both operands known to be f64 — direct float division.
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fdiv(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f, &nbc)
                    } else if op.fast_int.unwrap_or(false) {
                        // Python true division: int / int always returns float.
                        // Convert to f64 and do fdiv.
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_div", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let _one = builder.ins().iconst(types::I64, 1);
                        let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                        builder
                            .ins()
                            .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        // Python true division: int / int -> float.
                        let lhs_f = builder.ins().fcvt_from_sint(types::F64, lhs_val);
                        let rhs_f = builder.ins().fcvt_from_sint(types::F64, rhs_val);
                        let result_f = builder.ins().fdiv(lhs_f, rhs_f);
                        let fast_res = box_float_value(&mut builder, result_f, &nbc);
                        // Float result always valid — use iconst 1 for fits_inline.
                        let fits_inline = builder.ins().iconst(types::I8, 1);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_div", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let int_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, int_block, &[], slow_block, &[]);

                        builder.switch_to_block(int_block);
                        builder.seal_block(int_block);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                        let fast_block = builder.create_block();
                        builder
                            .ins()
                            .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        // Python true division: int / int -> float.
                        let lhs_f = builder.ins().fcvt_from_sint(types::F64, lhs_val);
                        let rhs_f = builder.ins().fcvt_from_sint(types::F64, rhs_val);
                        let result_f = builder.ins().fdiv(lhs_f, rhs_f);
                        let fast_res = box_float_value(&mut builder, result_f, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        // Inline float fast path: if both operands are floats, do f64 div directly.
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs, &nbc);
                        let float_block = builder.create_block();
                        let call_block = builder.create_block();
                        builder.set_cold_block(call_block);
                        builder
                            .ins()
                            .brif(both_flt, float_block, &[], call_block, &[]);

                        builder.switch_to_block(float_block);
                        builder.seal_block(float_block);
                        let lhs_ff = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_ff = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let flt_quot = builder.ins().fdiv(lhs_ff, rhs_ff);
                        let flt_res = box_float_value(&mut builder, flt_quot, &nbc);
                        jump_block(&mut builder, merge_block, &[flt_res]);

                        builder.switch_to_block(call_block);
                        builder.seal_block(call_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "floordiv" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_floordiv", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let one = builder.ins().iconst(types::I64, 1);
                        let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                        builder
                            .ins()
                            .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
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
                        let fast_res = box_int_value(&mut builder, floor_quot, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, floor_quot);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_floordiv", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let int_block = builder.create_block();
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, int_block, &[], slow_block, &[]);

                        builder.switch_to_block(int_block);
                        builder.seal_block(int_block);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                        builder
                            .ins()
                            .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
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
                        let fast_res = box_int_value(&mut builder, floor_quot, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, floor_quot);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "mod" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_mod", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                        builder
                            .ins()
                            .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let rem = builder.ins().srem(lhs_val, rhs_val);
                        let rem_nonzero = builder.ins().icmp(IntCC::NotEqual, rem, zero);
                        let lhs_neg = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, zero);
                        let rhs_neg = builder.ins().icmp(IntCC::SignedLessThan, rhs_val, zero);
                        let sign_diff = builder.ins().bxor(lhs_neg, rhs_neg);
                        let adjust = builder.ins().band(rem_nonzero, sign_diff);
                        let rem_adjusted = builder.ins().iadd(rem, rhs_val);
                        let mod_val = builder.ins().select(adjust, rem_adjusted, rem);
                        let fast_res = box_int_value(&mut builder, mod_val, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, mod_val);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_mod", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let int_block = builder.create_block();
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, int_block, &[], slow_block, &[]);

                        builder.switch_to_block(int_block);
                        builder.seal_block(int_block);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                        builder
                            .ins()
                            .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let rem = builder.ins().srem(lhs_val, rhs_val);
                        let rem_nonzero = builder.ins().icmp(IntCC::NotEqual, rem, zero);
                        let lhs_neg = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, zero);
                        let rhs_neg = builder.ins().icmp(IntCC::SignedLessThan, rhs_val, zero);
                        let sign_diff = builder.ins().bxor(lhs_neg, rhs_neg);
                        let adjust = builder.ins().band(rem_nonzero, sign_diff);
                        let rem_adjusted = builder.ins().iadd(rem, rhs_val);
                        let mod_val = builder.ins().select(adjust, rem_adjusted, rem);
                        let fast_res = box_int_value(&mut builder, mod_val, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, mod_val);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "floor_div" | "binop_floor_div" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        // Python floor_div: divide and floor towards negative infinity.
                        // sdiv truncates towards zero; we adjust when signs differ and
                        // there is a remainder.
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_floordiv", &[types::I64, types::I64], &[types::I64]);
                        let local_callee =
                            self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let rhs_nonzero =
                            builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                        builder
                            .ins()
                            .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let quot = builder.ins().sdiv(lhs_val, rhs_val);
                        let rem = builder.ins().srem(lhs_val, rhs_val);
                        // Adjust: if rem != 0 and signs of lhs/rhs differ, subtract 1.
                        let rem_nonzero =
                            builder.ins().icmp(IntCC::NotEqual, rem, zero);
                        let lhs_neg =
                            builder
                                .ins()
                                .icmp(IntCC::SignedLessThan, lhs_val, zero);
                        let rhs_neg =
                            builder
                                .ins()
                                .icmp(IntCC::SignedLessThan, rhs_val, zero);
                        let sign_diff = builder.ins().bxor(lhs_neg, rhs_neg);
                        let adjust = builder.ins().band(rem_nonzero, sign_diff);
                        let one = builder.ins().iconst(types::I64, 1);
                        let quot_adjusted = builder.ins().isub(quot, one);
                        let floor_val = builder.ins().select(adjust, quot_adjusted, quot);
                        let fast_res = box_int_value(&mut builder, floor_val, &nbc);
                        let fits_inline = int_value_fits_inline(&mut builder, floor_val);
                        brif_block(
                            &mut builder,
                            fits_inline,
                            merge_block,
                            &[fast_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_floordiv", &[types::I64, types::I64], &[types::I64]);
                        let local_callee =
                            self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        builder.inst_results(call)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "pow" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        // Inline pow for small non-negative exponents (0, 1, 2).
                        // Exponent >= 3 or negative falls back to runtime.
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_pow", &[types::I64, types::I64], &[types::I64]);
                        let local_callee =
                            self.module.declare_func_in_func(callee, builder.func);

                        let exp0_block = builder.create_block();
                        let exp1_block = builder.create_block();
                        let exp2_block = builder.create_block();
                        let exp2_fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let base_val = unbox_int(&mut builder, *lhs, &nbc);
                        let exp_val = unbox_int(&mut builder, *rhs, &nbc);

                        // Check exp == 0
                        let is_zero =
                            builder.ins().icmp_imm(IntCC::Equal, exp_val, 0);
                        builder
                            .ins()
                            .brif(is_zero, exp0_block, &[], exp1_block, &[]);

                        // exp == 0 → result is 1
                        builder.switch_to_block(exp0_block);
                        builder.seal_block(exp0_block);
                        let one = builder.ins().iconst(types::I64, 1);
                        let res_one = box_int_value(&mut builder, one, &nbc);
                        jump_block(&mut builder, merge_block, &[res_one]);

                        // Check exp == 1 → result is base (return lhs as-is)
                        builder.switch_to_block(exp1_block);
                        builder.seal_block(exp1_block);
                        let is_one =
                            builder.ins().icmp_imm(IntCC::Equal, exp_val, 1);
                        let exp1_ret_block = builder.create_block();
                        builder
                            .ins()
                            .brif(is_one, exp1_ret_block, &[], exp2_block, &[]);

                        builder.switch_to_block(exp1_ret_block);
                        builder.seal_block(exp1_ret_block);
                        jump_block(&mut builder, merge_block, &[*lhs]);

                        // Check exp == 2
                        builder.switch_to_block(exp2_block);
                        builder.seal_block(exp2_block);
                        let is_two =
                            builder.ins().icmp_imm(IntCC::Equal, exp_val, 2);
                        builder
                            .ins()
                            .brif(is_two, exp2_fast_block, &[], slow_block, &[]);

                        // exp == 2 → base * base with overflow check
                        builder.switch_to_block(exp2_fast_block);
                        builder.seal_block(exp2_fast_block);
                        let (sq, fits) = imul_checked_inline(&mut builder, base_val, base_val);
                        let sq_res = box_int_value(&mut builder, sq, &nbc);
                        brif_block(
                            &mut builder,
                            fits,
                            merge_block,
                            &[sq_res],
                            slow_block,
                            &[],
                        );

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_pow", &[types::I64, types::I64], &[types::I64]);
                        let local_callee =
                            self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        builder.inst_results(call)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "pow_mod" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let modulus = var_get(&mut builder, &vars, &args[2]).expect("Mod not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_pow_mod", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs, *modulus]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "round" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Round arg not found");
                    let ndigits =
                        var_get(&mut builder, &vars, &args[1]).expect("Round ndigits not found");
                    let has_ndigits = var_get(&mut builder, &vars, &args[2])
                        .expect("Round ndigits flag not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_round", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*val, *ndigits, *has_ndigits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "trunc" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Trunc arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_trunc", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "len" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    // Stack-tuple fast path: length is known at compile time.
                    if let Some(elems) = stack_tuples.get(&args[0]) {
                        let len_boxed = builder
                            .ins()
                            .iconst(types::I64, box_int(elems.len() as i64));
                        if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, len_boxed); }
                    } else if matches!(op.type_hint.as_deref(), Some("list") | Some("tuple")) {
                        // Inline fast path for list/tuple: read len from the
                        // underlying Vec<u64> without calling into the runtime.
                        //   raw_ptr  = unbox_ptr(val)
                        //   vec_ptr  = *(raw_ptr as *mut *mut Vec<u64>)  // first field of payload
                        //   len      = *(vec_ptr + 8)                   // Vec.len (second field)
                        //   result   = box_int(len)
                        let val = var_get(&mut builder, &vars, &args[0]).expect("Len arg not found");
                        let raw_ptr = unbox_ptr_value(&mut builder, *val, &nbc);
                        let vec_ptr = builder.ins().load(
                            types::I64,
                            MemFlags::trusted(),
                            raw_ptr,
                            0,
                        );
                        let len_val = builder.ins().load(
                            types::I64,
                            MemFlags::trusted(),
                            vec_ptr,
                            8, // offset to Vec::len (after the data pointer)
                        );
                        let res = box_int_value(&mut builder, len_val, &nbc);
                        if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                    } else {
                        let val = var_get(&mut builder, &vars, &args[0]).expect("Len arg not found");
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_len", &[types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*val]);
                        let res = builder.inst_results(call)[0];
                        if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                    }
                }
                "id" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Id arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_id", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "ord" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Ord arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_ord", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "chr" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Chr arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_chr", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "callargs_new" => {
                    let Some(out_name) = op.out else { continue; };
                    let zero = builder.ins().iconst(types::I64, 0);
                    let local_callee = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_callargs_new",
                        &[types::I64, types::I64],
                        &[types::I64],
                    );
                    let call = builder.ins().call(local_callee, &[zero, zero]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "list_new" => {
                    let empty_args: Vec<String> = Vec::new();
                    let args = op.args.as_ref().unwrap_or(&empty_args);
                    let Some(out_name) = op.out else { continue; };
                    let size = builder.ins().iconst(types::I64, box_int(args.len() as i64));

                    let new_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_builder_new", &[types::I64], &[types::I64]);
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let builder_ptr = builder.inst_results(new_call)[0];

                    let append_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_builder_append", &[types::I64, types::I64], &[]);
                    let append_local = self
                        .module
                        .declare_func_in_func(append_callee, builder.func);
                    for name in args {
                        let val = var_get(&mut builder, &vars, name).unwrap_or_else(|| {
                            panic!("List elem not found in {} op {}", func_ir.name, op_idx)
                        });
                        // Inc-ref each element so the builder owns its own
                        // reference.  The tracking system will dec-ref the
                        // caller's variable independently at its last use.
                        emit_inc_ref_obj(&mut builder, *val, local_inc_ref_obj, &nbc);
                        builder.ins().call(append_local, &[builder_ptr, *val]);
                    }

                    let finish_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_builder_finish", &[types::I64], &[types::I64]);
                    let finish_local = self
                        .module
                        .declare_func_in_func(finish_callee, builder.func);
                    let finish_call = builder.ins().call(finish_local, &[builder_ptr]);
                    let list_bits = builder.inst_results(finish_call)[0];
                    def_var_named(&mut builder, &vars, out_name, list_bits);
                }
                "callargs_push_pos" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let builder_ptr =
                        var_get(&mut builder, &vars, &args[0]).expect("Callargs builder not found");
                    let val =
                        var_get(&mut builder, &vars, &args[1]).expect("Callargs value not found");
                    let local_callee = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_callargs_push_pos",
                        &[types::I64, types::I64],
                        &[types::I64],
                    );
                    builder.ins().call(local_callee, &[*builder_ptr, *val]);
                }
                "callargs_push_kw" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let builder_ptr =
                        var_get(&mut builder, &vars, &args[0]).expect("Callargs builder not found");
                    let name =
                        var_get(&mut builder, &vars, &args[1]).expect("Callargs name not found");
                    let val =
                        var_get(&mut builder, &vars, &args[2]).expect("Callargs value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_callargs_push_kw", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder
                        .ins()
                        .call(local_callee, &[*builder_ptr, *name, *val]);
                }
                "callargs_expand_star" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let builder_ptr =
                        var_get(&mut builder, &vars, &args[0]).expect("Callargs builder not found");
                    let iterable = var_get(&mut builder, &vars, &args[1])
                        .expect("Callargs iterable not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_callargs_expand_star", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*builder_ptr, *iterable]);
                }
                "callargs_expand_kwstar" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let builder_ptr =
                        var_get(&mut builder, &vars, &args[0]).expect("Callargs builder not found");
                    let mapping =
                        var_get(&mut builder, &vars, &args[1]).expect("Callargs mapping not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_callargs_expand_kwstar", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*builder_ptr, *mapping]);
                }
                "range_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let start =
                        var_get(&mut builder, &vars, &args[0]).expect("Range start not found");
                    let stop =
                        var_get(&mut builder, &vars, &args[1]).expect("Range stop not found");
                    let step =
                        var_get(&mut builder, &vars, &args[2]).expect("Range step not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_range_new", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*start, *stop, *step]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "list_from_range" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let start = var_get(&mut builder, &vars, &args[0])
                        .expect("List-from-range start not found");
                    let stop = var_get(&mut builder, &vars, &args[1])
                        .expect("List-from-range stop not found");
                    let step = var_get(&mut builder, &vars, &args[2])
                        .expect("List-from-range step not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_from_range", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*start, *stop, *step]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "tuple_new" => {
                    let empty_args: Vec<String> = Vec::new();
                    let args = op.args.as_ref().unwrap_or(&empty_args);
                    let Some(out_name) = op.out else { continue; };

                    if op.stack_eligible == Some(true) && args.len() <= 4 {
                        // Stack-eligible fast path: allocate tuple on the
                        // Cranelift stack frame instead of calling the runtime
                        // heap allocator.  Layout mirrors MoltHeader (40 bytes)
                        // followed by n packed i64 element slots.
                        //
                        // The element Values are also kept in `stack_tuples`
                        // so that `index` and `len` ops can resolve them at
                        // compile time without any memory loads.
                        let n = args.len();
                        let slot_bytes = (HEADER_SIZE_BYTES as usize) + n * 8;
                        let slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            slot_bytes as u32,
                            3, // align_shift: 2^3 = 8-byte alignment
                        ));
                        // Data pointer = slot base + HEADER_SIZE_BYTES.
                        // All header fields are addressed via negative offsets
                        // from data_ptr, matching the runtime MoltHeader layout.
                        let data_ptr = builder.ins().stack_addr(
                            types::I64,
                            slot,
                            HEADER_SIZE_BYTES,
                        );

                        // Initialize header fields (offsets relative to data_ptr).
                        // type_id (u32 at -16)
                        let type_id_val = builder.ins().iconst(types::I32, 206); // TYPE_ID_TUPLE
                        builder.ins().store(
                            MemFlags::trusted(),
                            type_id_val,
                            data_ptr,
                            -HEADER_SIZE_BYTES,
                        );
                        // ref_count (u32 at -12) — set to u32::MAX (immortal)
                        // so the runtime never frees the stack memory.
                        let rc_val = builder.ins().iconst(types::I32, u32::MAX as i64);
                        builder.ins().store(
                            MemFlags::trusted(),
                            rc_val,
                            data_ptr,
                            HEADER_REFCOUNT_OFFSET,
                        );
                        // flags (u32 at -8) — set HEADER_FLAG_IMMORTAL
                        let flags_val = builder
                            .ins()
                            .iconst(types::I32, HEADER_FLAG_IMMORTAL as i64);
                        builder.ins().store(
                            MemFlags::trusted(),
                            flags_val,
                            data_ptr,
                            HEADER_FLAGS_OFFSET,
                        );
                        // size_class (u16 at -4) and cold_idx (u16 at -2)
                        // are left as zero: stack tuples have no cold header
                        // and zero size_class means "oversized" (exact size
                        // irrelevant since the slot is never freed).
                        //
                        // NOTE: The old 40-byte header had poll_fn at -32,
                        // state at -24, and extended_size at -16.  Those
                        // fields were moved to MoltColdHeader; writing them
                        // here would corrupt adjacent stack slots and—
                        // critically—overwrite the type_id we just stored
                        // (the -16 store clobbered type_id with the byte
                        // size, making tuples appear as type_id 32/48/etc.).

                        // Store elements and collect Values for stack_tuples.
                        let mut elems: Vec<Value> = Vec::with_capacity(n);
                        for (i, name) in args.iter().enumerate() {
                            let val = var_get(&mut builder, &vars, name)
                                .expect("Stack tuple elem not found");
                            elems.push(*val);
                            builder.ins().store(
                                MemFlags::trusted(),
                                *val,
                                data_ptr,
                                (i * 8) as i32,
                            );
                        }
                        stack_tuples.insert(out_name.to_string(), elems);

                        // Box the data pointer as a NaN-boxed pointer so the
                        // variable holds a usable value (not a zero sentinel).
                        let boxed = box_ptr_value(&mut builder, data_ptr, &nbc);
                        def_var_named(&mut builder, &vars, out_name, boxed);
                    } else {
                        let size = builder.ins().iconst(types::I64, box_int(args.len() as i64));

                        let new_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_builder_new", &[types::I64], &[types::I64]);
                        let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                        let new_call = builder.ins().call(new_local, &[size]);
                        let builder_ptr = builder.inst_results(new_call)[0];

                        let append_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_builder_append", &[types::I64, types::I64], &[]);
                        let append_local = self
                            .module
                            .declare_func_in_func(append_callee, builder.func);
                        for name in args {
                            let val = var_get(&mut builder, &vars, name).expect("Tuple elem not found");
                            // Inc-ref each element so the builder owns its own
                            // reference.
                            emit_inc_ref_obj(&mut builder, *val, local_inc_ref_obj, &nbc);
                            builder.ins().call(append_local, &[builder_ptr, *val]);
                        }

                        let finish_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_tuple_builder_finish", &[types::I64], &[types::I64]);
                        let finish_local = self
                            .module
                            .declare_func_in_func(finish_callee, builder.func);
                        let finish_call = builder.ins().call(finish_local, &[builder_ptr]);
                        let tuple_bits = builder.inst_results(finish_call)[0];
                        def_var_named(&mut builder, &vars, out_name, tuple_bits);
                    }
                }
                "unpack_sequence" => {
                    // Outlined sequence unpacking: args[0] is the sequence,
                    // args[1..] are the output variable names.
                    // op.value holds the expected element count.
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq_val = var_get(&mut builder, &vars, &args[0])
                        .expect("Unpack sequence source not found");
                    let expected_count = op.value.unwrap_or(0) as usize;

                    // Allocate a stack slot for the output array.
                    let slot_size = std::cmp::max(expected_count, 1) * 8;
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        slot_size as u32,
                        3, // align_shift: 2^3 = 8-byte alignment
                    ));
                    let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);

                    let expected_val =
                        builder.ins().iconst(types::I64, expected_count as i64);

                    // Call molt_unpack_sequence(seq_bits, expected_count, output_ptr) -> u64
                    let unpack_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_unpack_sequence",
                        &[types::I64, types::I64, types::I64],
                        &[types::I64],
                    );
                    builder
                        .ins()
                        .call(unpack_local, &[*seq_val, expected_val, out_ptr]);

                    // Load each element from the output array into its named variable.
                    for i in 0..expected_count {
                        let elem = builder
                            .ins()
                            .stack_load(types::I64, out_slot, (i * 8) as i32);
                        def_var_named(&mut builder, &vars, &args[1 + i], elem);
                    }
                }
                "list_append" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let val = var_get(&mut builder, &vars, &args[1])
                        .expect("List append value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_append", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "list_pop" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let idx =
                        var_get(&mut builder, &vars, &args[1]).expect("List pop index not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_pop", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *idx]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "list_extend" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let other = var_get(&mut builder, &vars, &args[1])
                        .expect("List extend iterable not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_extend", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *other]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "list_insert" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let idx = var_get(&mut builder, &vars, &args[1])
                        .expect("List insert index not found");
                    let val = var_get(&mut builder, &vars, &args[2])
                        .expect("List insert value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_insert", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *idx, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "list_remove" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let val = var_get(&mut builder, &vars, &args[1])
                        .expect("List remove value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_remove", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "list_clear" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_clear", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "list_copy" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_copy", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "list_reverse" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_reverse", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "list_count" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let val =
                        var_get(&mut builder, &vars, &args[1]).expect("List count value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_count", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "list_index" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let val =
                        var_get(&mut builder, &vars, &args[1]).expect("List index value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_index", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "list_index_range" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let val =
                        var_get(&mut builder, &vars, &args[1]).expect("List index value not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("List index start not found");
                    let stop =
                        var_get(&mut builder, &vars, &args[3]).expect("List index stop not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_list_index_range", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*list, *val, *start, *stop]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "tuple_from_list" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let list =
                        var_get(&mut builder, &vars, &args[0]).expect("Tuple source not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_tuple_from_list", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_new" => {
                    let empty_args: Vec<String> = Vec::new();
                    let args = op.args.as_ref().unwrap_or(&empty_args);
                    let Some(out_name) = op.out else { continue; };
                    let size = builder.ins().iconst(types::I64, (args.len() / 2) as i64);

                    let new_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_new", &[types::I64], &[types::I64]);
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let dict_bits = builder.inst_results(new_call)[0];

                    let set_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_set", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let set_local = self.module.declare_func_in_func(set_callee, builder.func);
                    let mut current = dict_bits;
                    for pair in args.chunks(2) {
                        let key =
                            var_get(&mut builder, &vars, &pair[0]).expect("Dict key not found");
                        let val =
                            var_get(&mut builder, &vars, &pair[1]).expect("Dict val not found");
                        let set_call = builder.ins().call(set_local, &[current, *key, *val]);
                        current = builder.inst_results(set_call)[0];
                    }
                    def_var_named(&mut builder, &vars, out_name, current);
                }
                "dict_from_obj" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Dict source not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_from_obj", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "set_new" => {
                    let empty_args: Vec<String> = Vec::new();
                    let args = op.args.as_ref().unwrap_or(&empty_args);
                    let Some(out_name) = op.out else { continue; };
                    let size = builder.ins().iconst(types::I64, args.len() as i64);

                    let new_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_set_new", &[types::I64], &[types::I64]);
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let set_bits = builder.inst_results(new_call)[0];

                    if !args.is_empty() {
                        let add_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_set_add", &[types::I64, types::I64], &[types::I64]);
                        let add_local = self.module.declare_func_in_func(add_callee, builder.func);
                        for name in args {
                            let val = var_get(&mut builder, &vars, name).unwrap_or_else(|| {
                                panic!("Set elem not found in {} op {}", func_ir.name, op_idx)
                            });
                            builder.ins().call(add_local, &[set_bits, *val]);
                        }
                    }

                    def_var_named(&mut builder, &vars, out_name, set_bits);
                }
                "frozenset_new" => {
                    let empty_args: Vec<String> = Vec::new();
                    let args = op.args.as_ref().unwrap_or(&empty_args);
                    let Some(out_name) = op.out else { continue; };
                    let size = builder.ins().iconst(types::I64, args.len() as i64);

                    let new_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_frozenset_new", &[types::I64], &[types::I64]);
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let set_bits = builder.inst_results(new_call)[0];

                    if !args.is_empty() {
                        let add_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_frozenset_add", &[types::I64, types::I64], &[types::I64]);
                        let add_local = self.module.declare_func_in_func(add_callee, builder.func);
                        for name in args {
                            let val = var_get(&mut builder, &vars, name).unwrap_or_else(|| {
                                panic!("Frozenset elem not found in {} op {}", func_ir.name, op_idx)
                            });
                            builder.ins().call(add_local, &[set_bits, *val]);
                        }
                    }

                    def_var_named(&mut builder, &vars, out_name, set_bits);
                }
                "dict_get" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let key = var_get(&mut builder, &vars, &args[1]).expect("Dict key not found");
                    let default =
                        var_get(&mut builder, &vars, &args[2]).expect("Dict default not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_get", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *key, *default]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_inc" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let key = var_get(&mut builder, &vars, &args[1]).expect("Dict key not found");
                    let delta = var_get(&mut builder, &vars, &args[2])
                        .expect("Dict increment value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_inc", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *key, *delta]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_str_int_inc" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let key = var_get(&mut builder, &vars, &args[1]).expect("Dict key not found");
                    let delta = var_get(&mut builder, &vars, &args[2])
                        .expect("Dict increment value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_str_int_inc", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *key, *delta]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_split_ws_dict_inc" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let line = var_get(&mut builder, &vars, &args[0]).expect("Line not found");
                    let dict = var_get(&mut builder, &vars, &args[1]).expect("Dict not found");
                    let delta = var_get(&mut builder, &vars, &args[2]).expect("Delta not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_split_ws_dict_inc", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*line, *dict, *delta]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "taq_ingest_line" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let line = var_get(&mut builder, &vars, &args[1]).expect("Line not found");
                    let bucket_size =
                        var_get(&mut builder, &vars, &args[2]).expect("Bucket size not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_taq_ingest_line", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*dict, *line, *bucket_size]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_split_sep_dict_inc" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let line = var_get(&mut builder, &vars, &args[0]).expect("Line not found");
                    let sep = var_get(&mut builder, &vars, &args[1]).expect("Separator not found");
                    let dict = var_get(&mut builder, &vars, &args[2]).expect("Dict not found");
                    let delta = var_get(&mut builder, &vars, &args[3]).expect("Delta not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_split_sep_dict_inc", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*line, *sep, *dict, *delta]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_pop" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let key = var_get(&mut builder, &vars, &args[1]).expect("Dict key not found");
                    let default =
                        var_get(&mut builder, &vars, &args[2]).expect("Dict default not found");
                    let has_default = var_get(&mut builder, &vars, &args[3])
                        .expect("Dict default flag not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_pop", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*dict, *key, *default, *has_default]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_setdefault" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let key = var_get(&mut builder, &vars, &args[1]).expect("Dict key not found");
                    let default =
                        var_get(&mut builder, &vars, &args[2]).expect("Dict default not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_setdefault", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *key, *default]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_setdefault_empty_list" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let key = var_get(&mut builder, &vars, &args[1]).expect("Dict key not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_setdefault_empty_list", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *key]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_update" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let other = var_get(&mut builder, &vars, &args[1])
                        .expect("Dict update iterable not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_update", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *other]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_clear" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_clear", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_copy" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_copy", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_popitem" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_popitem", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_update_kwstar" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let other = var_get(&mut builder, &vars, &args[1])
                        .expect("Dict update mapping not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_update_kwstar", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *other]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "set_add" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let key_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Set key not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_set_add", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *key_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "frozenset_add" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let set_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Frozenset not found");
                    let key_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Frozenset key not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_frozenset_add", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *key_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "set_discard" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let key_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Set key not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_set_discard", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *key_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "set_remove" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let key_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Set key not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_set_remove", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *key_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "set_pop" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_set_pop", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "set_update" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let other_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Set update arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_set_update", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *other_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "set_intersection_update" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let other_bits = var_get(&mut builder, &vars, &args[1])
                        .expect("Set intersection update arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_set_intersection_update", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *other_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "set_difference_update" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let other_bits = var_get(&mut builder, &vars, &args[1])
                        .expect("Set difference update arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_set_difference_update", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *other_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "set_symdiff_update" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let other_bits = var_get(&mut builder, &vars, &args[1])
                        .expect("Set symdiff update arg not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_set_symdiff_update", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *other_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_keys" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_keys", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_values" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_values", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_items" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_items", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "tuple_count" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let tuple = var_get(&mut builder, &vars, &args[0]).expect("Tuple not found");
                    let val = var_get(&mut builder, &vars, &args[1])
                        .expect("Tuple count value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_tuple_count", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*tuple, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "tuple_index" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let tuple = var_get(&mut builder, &vars, &args[0]).expect("Tuple not found");
                    let val = var_get(&mut builder, &vars, &args[1])
                        .expect("Tuple index value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_tuple_index", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*tuple, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "iter" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Iter source not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_iter_checked", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "enumerate" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let iterable = var_get(&mut builder, &vars, &args[0])
                        .expect("Enumerate iterable not found");
                    let start =
                        var_get(&mut builder, &vars, &args[1]).expect("Enumerate start not found");
                    let has_start = var_get(&mut builder, &vars, &args[2])
                        .expect("Enumerate has_start not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_enumerate", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*iterable, *start, *has_start]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "aiter" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0])
                        .expect("Async iter source not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_aiter", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "iter_next" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let iter = var_get(&mut builder, &vars, &args[0]).expect("Iter not found");
                    let pair_name = op.out.clone().unwrap();

                    // Peephole: detect the iter_next → index(pair,1) → ... → index(pair,0)
                    // pattern emitted by for-loops and replace with a single
                    // molt_iter_next_unboxed call that avoids the tuple allocation and
                    // two molt_index dispatches.
                    let mut done_idx = None;
                    let mut val_idx = None;
                    // Scan ahead for INDEX ops that reference our pair.  Use a
                    // wider window (16 ops) to bridge exception-handling
                    // boilerplate (check_exception, inc_ref, etc.) that can
                    // separate iter_next from its index consumers.
                    let scan_limit = (op_idx + 16).min(ops.len());
                    for peek in (op_idx + 1)..scan_limit {
                        if skip_ops.contains(&peek) {
                            continue;
                        }
                        let peek_op = &ops[peek];
                        if peek_op.kind == "index" {
                            if let Some(ref pargs) = peek_op.args {
                                if pargs.len() >= 2 && pargs[0] == pair_name {
                                    // Check if the index argument is a const "1" or "0".
                                    // The const var names are looked up by scanning
                                    // backwards for a const op that defined the arg.
                                    let idx_var = &pargs[1];
                                    // Find the const op that produced idx_var.
                                    if let Some(const_val) = Self::resolve_const_int(ops, peek, idx_var) {
                                        if const_val == 1 && done_idx.is_none() {
                                            done_idx = Some(peek);
                                        } else if const_val == 0 && val_idx.is_none() {
                                            val_idx = Some(peek);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if let (Some(di), Some(vi)) = (done_idx, val_idx) {
                        // === Unboxed fast path ===
                        // Allocate a stack slot for the yielded value.
                        let val_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let val_ptr = builder.ins().stack_addr(types::I64, val_slot, 0);

                        // Call molt_iter_next_unboxed(iter, &value_out) → done_flag (MoltObject bool)
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_iter_next_unboxed", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*iter, val_ptr]);
                        let done_bits = builder.inst_results(call)[0];

                        // Load the value from the stack slot.
                        let loaded_val = builder.ins().load(types::I64, MemFlags::trusted(), val_ptr, 0);

                        // The done_bits is the MoltObject bool that index(pair,1) would return.
                        let done_out = ops[di].out.clone().unwrap();
                        def_var_named(&mut builder, &vars, done_out, done_bits);

                        // The loaded_val is the value that index(pair,0) would return.
                        let val_out = ops[vi].out.clone().unwrap();
                        def_var_named(&mut builder, &vars, val_out, loaded_val);

                        // Also define the pair variable (as the done flag) so that any
                        // exception-check referencing pair still works.
                        def_var_named(&mut builder, &vars, pair_name, done_bits);

                        // Mark the two INDEX ops as skipped.
                        skip_ops.insert(di);
                        skip_ops.insert(vi);
                    } else {
                        // === Fallback: original boxed path ===
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_iter_next", &[types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*iter]);
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, pair_name, res);
                    }
                }
                "anext" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let iter =
                        var_get(&mut builder, &vars, &args[0]).expect("Async iter not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_anext", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*iter]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "asyncgen_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let gen_obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Generator not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_asyncgen_new", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*gen_obj]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "asyncgen_shutdown" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_asyncgen_shutdown", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "gen_send" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let gen_obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Generator not found");
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Send value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_generator_send", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*gen_obj, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "gen_throw" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let gen_obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Generator not found");
                    let val =
                        var_get(&mut builder, &vars, &args[1]).expect("Throw value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_generator_throw", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*gen_obj, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "gen_close" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let gen_obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Generator not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_generator_close", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*gen_obj]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "is_generator" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Obj not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_is_generator", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "is_bound_method" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Obj not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_is_bound_method", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "is_callable" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Obj not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_is_callable", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "index" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    // Stack-tuple fast path: resolve element at compile time.
                    let stack_resolved = stack_tuples.get(&args[0]).and_then(|elems| {
                        Self::resolve_const_int(ops, op_idx, &args[1]).and_then(|ci| {
                            let ui = ci as usize;
                            elems.get(ui).copied()
                        })
                    });
                    if let Some(elem_val) = stack_resolved {
                        // The element came from a non-escaping tuple; inc_ref
                        // to keep refcount correct since the tuple itself was
                        // never heap-allocated.
                        emit_inc_ref_obj(&mut builder, elem_val, local_inc_ref_obj, &nbc);
                        if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, elem_val); }
                    } else {
                        let obj = var_get(&mut builder, &vars, &args[0]).expect("Obj not found");
                        let idx = var_get(&mut builder, &vars, &args[1]).expect("Index not found");
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        // fast_int: index is a known NaN-boxed int; use the
                        // list-specific fast path which avoids full type dispatch.
                        let fn_name = if op.fast_int.unwrap_or(false) {
                            "molt_list_getitem_int_fast"
                        } else {
                            "molt_index"
                        };
                        let callee = self
                            .module
                            .declare_function(fn_name, Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*obj, *idx]);
                        let res = builder.inst_results(call)[0];
                        if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                    }
                }
                "store_index" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Obj not found in {} op {}", func_ir.name, op_idx)
                    });
                    let idx = var_get(&mut builder, &vars, &args[1]).unwrap_or_else(|| {
                        panic!("Index not found in {} op {}", func_ir.name, op_idx)
                    });
                    let val = var_get(&mut builder, &vars, &args[2]).unwrap_or_else(|| {
                        panic!("Value not found in {} op {}", func_ir.name, op_idx)
                    });
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_store_index", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_set" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict_bits = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Dict not found in {} op {}", func_ir.name, op_idx)
                    });
                    let key_bits = var_get(&mut builder, &vars, &args[1]).unwrap_or_else(|| {
                        panic!("Key not found in {} op {}", func_ir.name, op_idx)
                    });
                    let val_bits = var_get(&mut builder, &vars, &args[2]).unwrap_or_else(|| {
                        panic!("Value not found in {} op {}", func_ir.name, op_idx)
                    });
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_set", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*dict_bits, *key_bits, *val_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dict_update_missing" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let dict_bits = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Dict not found in {} op {}", func_ir.name, op_idx)
                    });
                    let key_bits = var_get(&mut builder, &vars, &args[1]).unwrap_or_else(|| {
                        panic!("Key not found in {} op {}", func_ir.name, op_idx)
                    });
                    let val_bits = var_get(&mut builder, &vars, &args[2]).unwrap_or_else(|| {
                        panic!("Value not found in {} op {}", func_ir.name, op_idx)
                    });
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dict_update_missing", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*dict_bits, *key_bits, *val_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "del_index" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Obj not found in {} op {}", func_ir.name, op_idx)
                    });
                    let idx = var_get(&mut builder, &vars, &args[1]).unwrap_or_else(|| {
                        panic!("Index not found in {} op {}", func_ir.name, op_idx)
                    });
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_del_index", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let target =
                        var_get(&mut builder, &vars, &args[0]).expect("Slice target not found");
                    let start =
                        var_get(&mut builder, &vars, &args[1]).expect("Slice start not found");
                    let end = var_get(&mut builder, &vars, &args[2]).expect("Slice end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_slice", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*target, *start, *end]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "slice_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let start =
                        var_get(&mut builder, &vars, &args[0]).expect("Slice start not found");
                    let stop =
                        var_get(&mut builder, &vars, &args[1]).expect("Slice stop not found");
                    let step =
                        var_get(&mut builder, &vars, &args[2]).expect("Slice step not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_slice_new", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*start, *stop, *step]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytes_find" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Find haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Find needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_find", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytes_find_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Find haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Find needle not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Find start not found");
                    let end = var_get(&mut builder, &vars, &args[3]).expect("Find end not found");
                    let has_start =
                        var_get(&mut builder, &vars, &args[4]).expect("Find has_start not found");
                    let has_end =
                        var_get(&mut builder, &vars, &args[5]).expect("Find has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_find_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytearray_find" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Find haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Find needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytearray_find", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytearray_find_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Find haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Find needle not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Find start not found");
                    let end = var_get(&mut builder, &vars, &args[3]).expect("Find end not found");
                    let has_start =
                        var_get(&mut builder, &vars, &args[4]).expect("Find has_start not found");
                    let has_end =
                        var_get(&mut builder, &vars, &args[5]).expect("Find has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytearray_find_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_find" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Find haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Find needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_find", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_find_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Find haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Find needle not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Find start not found");
                    let end = var_get(&mut builder, &vars, &args[3]).expect("Find end not found");
                    let has_start =
                        var_get(&mut builder, &vars, &args[4]).expect("Find has_start not found");
                    let has_end =
                        var_get(&mut builder, &vars, &args[5]).expect("Find has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_find_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_format" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val =
                        var_get(&mut builder, &vars, &args[0]).expect("Format value not found");
                    let spec =
                        var_get(&mut builder, &vars, &args[1]).expect("Format spec not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_format_builtin", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val, *spec]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_startswith" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Startswith haystack not found");
                    let needle = var_get(&mut builder, &vars, &args[1])
                        .expect("Startswith needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_startswith", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_startswith_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Startswith haystack not found");
                    let needle = var_get(&mut builder, &vars, &args[1])
                        .expect("Startswith needle not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Startswith start not found");
                    let end =
                        var_get(&mut builder, &vars, &args[3]).expect("Startswith end not found");
                    let has_start = var_get(&mut builder, &vars, &args[4])
                        .expect("Startswith has_start not found");
                    let has_end = var_get(&mut builder, &vars, &args[5])
                        .expect("Startswith has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_startswith_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytes_startswith" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Startswith haystack not found");
                    let needle = var_get(&mut builder, &vars, &args[1])
                        .expect("Startswith needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_startswith", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytes_startswith_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Startswith haystack not found");
                    let needle = var_get(&mut builder, &vars, &args[1])
                        .expect("Startswith needle not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Startswith start not found");
                    let end =
                        var_get(&mut builder, &vars, &args[3]).expect("Startswith end not found");
                    let has_start = var_get(&mut builder, &vars, &args[4])
                        .expect("Startswith has_start not found");
                    let has_end = var_get(&mut builder, &vars, &args[5])
                        .expect("Startswith has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_startswith_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytearray_startswith" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Startswith haystack not found");
                    let needle = var_get(&mut builder, &vars, &args[1])
                        .expect("Startswith needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytearray_startswith", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytearray_startswith_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Startswith haystack not found");
                    let needle = var_get(&mut builder, &vars, &args[1])
                        .expect("Startswith needle not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Startswith start not found");
                    let end =
                        var_get(&mut builder, &vars, &args[3]).expect("Startswith end not found");
                    let has_start = var_get(&mut builder, &vars, &args[4])
                        .expect("Startswith has_start not found");
                    let has_end = var_get(&mut builder, &vars, &args[5])
                        .expect("Startswith has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytearray_startswith_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_endswith" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Endswith haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Endswith needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_endswith", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_endswith_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Endswith haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Endswith needle not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Endswith start not found");
                    let end =
                        var_get(&mut builder, &vars, &args[3]).expect("Endswith end not found");
                    let has_start = var_get(&mut builder, &vars, &args[4])
                        .expect("Endswith has_start not found");
                    let has_end =
                        var_get(&mut builder, &vars, &args[5]).expect("Endswith has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_endswith_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytes_endswith" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Endswith haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Endswith needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_endswith", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytes_endswith_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Endswith haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Endswith needle not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Endswith start not found");
                    let end =
                        var_get(&mut builder, &vars, &args[3]).expect("Endswith end not found");
                    let has_start = var_get(&mut builder, &vars, &args[4])
                        .expect("Endswith has_start not found");
                    let has_end =
                        var_get(&mut builder, &vars, &args[5]).expect("Endswith has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_endswith_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytearray_endswith" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Endswith haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Endswith needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytearray_endswith", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytearray_endswith_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Endswith haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Endswith needle not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Endswith start not found");
                    let end =
                        var_get(&mut builder, &vars, &args[3]).expect("Endswith end not found");
                    let has_start = var_get(&mut builder, &vars, &args[4])
                        .expect("Endswith has_start not found");
                    let has_end =
                        var_get(&mut builder, &vars, &args[5]).expect("Endswith has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytearray_endswith_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_count" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Count haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Count needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_count", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytes_count" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Count haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Count needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_count", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytearray_count" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Count haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Count needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytearray_count", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_count_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Count haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Count needle not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Count start not found");
                    let end = var_get(&mut builder, &vars, &args[3]).expect("Count end not found");
                    let has_start =
                        var_get(&mut builder, &vars, &args[4]).expect("Count has_start not found");
                    let has_end =
                        var_get(&mut builder, &vars, &args[5]).expect("Count has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_count_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytes_count_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Count haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Count needle not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Count start not found");
                    let end = var_get(&mut builder, &vars, &args[3]).expect("Count end not found");
                    let has_start =
                        var_get(&mut builder, &vars, &args[4]).expect("Count has_start not found");
                    let has_end =
                        var_get(&mut builder, &vars, &args[5]).expect("Count has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_count_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytearray_count_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Count haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Count needle not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Count start not found");
                    let end = var_get(&mut builder, &vars, &args[3]).expect("Count end not found");
                    let has_start =
                        var_get(&mut builder, &vars, &args[4]).expect("Count has_start not found");
                    let has_end =
                        var_get(&mut builder, &vars, &args[5]).expect("Count has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytearray_count_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "env_get" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let key = var_get(&mut builder, &vars, &args[0]).expect("Env key not found");
                    let default =
                        var_get(&mut builder, &vars, &args[1]).expect("Env default not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_env_get", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*key, *default]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_join" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let sep =
                        var_get(&mut builder, &vars, &args[0]).expect("Join separator not found");
                    let items =
                        var_get(&mut builder, &vars, &args[1]).expect("Join items not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_join", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*sep, *items]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_split" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Split haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Split needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_split", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_split_max" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Split haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Split needle not found");
                    let maxsplit =
                        var_get(&mut builder, &vars, &args[2]).expect("Split maxsplit not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_split_max", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *maxsplit]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "statistics_mean_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0])
                        .expect("Statistics mean slice sequence not found");
                    let start = var_get(&mut builder, &vars, &args[1])
                        .expect("Statistics mean slice start not found");
                    let end = var_get(&mut builder, &vars, &args[2])
                        .expect("Statistics mean slice end not found");
                    let has_start = var_get(&mut builder, &vars, &args[3])
                        .expect("Statistics mean slice has_start not found");
                    let has_end = var_get(&mut builder, &vars, &args[4])
                        .expect("Statistics mean slice has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_statistics_mean_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*seq, *start, *end, *has_start, *has_end]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "statistics_stdev_slice" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let seq = var_get(&mut builder, &vars, &args[0])
                        .expect("Statistics stdev slice sequence not found");
                    let start = var_get(&mut builder, &vars, &args[1])
                        .expect("Statistics stdev slice start not found");
                    let end = var_get(&mut builder, &vars, &args[2])
                        .expect("Statistics stdev slice end not found");
                    let has_start = var_get(&mut builder, &vars, &args[3])
                        .expect("Statistics stdev slice has_start not found");
                    let has_end = var_get(&mut builder, &vars, &args[4])
                        .expect("Statistics stdev slice has_end not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_statistics_stdev_slice", &[types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*seq, *start, *end, *has_start, *has_end]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_lower" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Lower string not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_lower", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_upper" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Upper string not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_upper", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_capitalize" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Capitalize string not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_capitalize", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_strip" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Strip string not found");
                    let chars =
                        var_get(&mut builder, &vars, &args[1]).expect("Strip chars not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_strip", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *chars]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_lstrip" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Lstrip string not found");
                    let chars =
                        var_get(&mut builder, &vars, &args[1]).expect("Lstrip chars not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_lstrip", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *chars]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_rstrip" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Rstrip string not found");
                    let chars =
                        var_get(&mut builder, &vars, &args[1]).expect("Rstrip chars not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_rstrip", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *chars]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_replace" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Replace haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Replace needle not found");
                    let replacement = var_get(&mut builder, &vars, &args[2])
                        .expect("Replace replacement not found");
                    let count =
                        var_get(&mut builder, &vars, &args[3]).expect("Replace count not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_replace", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *replacement, *count]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytes_split" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Split haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Split needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_split", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytes_split_max" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Split haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Split needle not found");
                    let maxsplit =
                        var_get(&mut builder, &vars, &args[2]).expect("Split maxsplit not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_split_max", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *maxsplit]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytearray_split" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Split haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Split needle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytearray_split", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytearray_split_max" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Split haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Split needle not found");
                    let maxsplit =
                        var_get(&mut builder, &vars, &args[2]).expect("Split maxsplit not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytearray_split_max", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *maxsplit]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytes_replace" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Replace haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Replace needle not found");
                    let replacement = var_get(&mut builder, &vars, &args[2])
                        .expect("Replace replacement not found");
                    let count =
                        var_get(&mut builder, &vars, &args[3]).expect("Replace count not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_replace", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *replacement, *count]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytearray_replace" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Replace haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Replace needle not found");
                    let replacement = var_get(&mut builder, &vars, &args[2])
                        .expect("Replace replacement not found");
                    let count =
                        var_get(&mut builder, &vars, &args[3]).expect("Replace count not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytearray_replace", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *replacement, *count]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytes_from_obj" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Bytes source not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_from_obj", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytes_from_str" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Bytes source not found");
                    let encoding =
                        var_get(&mut builder, &vars, &args[1]).expect("Bytes encoding not found");
                    let errors =
                        var_get(&mut builder, &vars, &args[2]).expect("Bytes errors not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytes_from_str", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*src, *encoding, *errors]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytearray_from_obj" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Bytearray source not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytearray_from_obj", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bytearray_from_str" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Bytearray source not found");
                    let encoding = var_get(&mut builder, &vars, &args[1])
                        .expect("Bytearray encoding not found");
                    let errors =
                        var_get(&mut builder, &vars, &args[2]).expect("Bytearray errors not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bytearray_from_str", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*src, *encoding, *errors]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "float_from_obj" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Float source not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_float_from_obj", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "int_from_obj" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Int value not found");
                    let base = var_get(&mut builder, &vars, &args[1]).expect("Int base not found");
                    let has_base =
                        var_get(&mut builder, &vars, &args[2]).expect("Int base flag not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_int_from_obj", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val, *base, *has_base]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "complex_from_obj" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val =
                        var_get(&mut builder, &vars, &args[0]).expect("Complex value not found");
                    let imag =
                        var_get(&mut builder, &vars, &args[1]).expect("Complex imag not found");
                    let has_imag =
                        var_get(&mut builder, &vars, &args[2]).expect("Complex flag not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_complex_from_obj", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val, *imag, *has_imag]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "intarray_from_seq" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Intarray source not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_intarray_from_seq", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "memoryview_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let src = var_get(&mut builder, &vars, &args[0])
                        .expect("Memoryview source not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_memoryview_new", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "memoryview_tobytes" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Memoryview value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_memoryview_tobytes", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "memoryview_cast" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let view =
                        var_get(&mut builder, &vars, &args[0]).expect("Memoryview not found");
                    let format = var_get(&mut builder, &vars, &args[1])
                        .expect("Memoryview format not found");
                    let shape =
                        var_get(&mut builder, &vars, &args[2]).expect("Memoryview shape not found");
                    let has_shape = var_get(&mut builder, &vars, &args[3])
                        .expect("Memoryview shape flag not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_memoryview_cast", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*view, *format, *shape, *has_shape]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "buffer2d_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let rows =
                        var_get(&mut builder, &vars, &args[0]).expect("Buffer2D rows not found");
                    let cols =
                        var_get(&mut builder, &vars, &args[1]).expect("Buffer2D cols not found");
                    let init =
                        var_get(&mut builder, &vars, &args[2]).expect("Buffer2D init not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_buffer2d_new", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*rows, *cols, *init]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "buffer2d_get" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let buf = var_get(&mut builder, &vars, &args[0]).expect("Buffer2D not found");
                    let row =
                        var_get(&mut builder, &vars, &args[1]).expect("Buffer2D row not found");
                    let col =
                        var_get(&mut builder, &vars, &args[2]).expect("Buffer2D col not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_buffer2d_get", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*buf, *row, *col]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "buffer2d_set" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let buf = var_get(&mut builder, &vars, &args[0]).expect("Buffer2D not found");
                    let row =
                        var_get(&mut builder, &vars, &args[1]).expect("Buffer2D row not found");
                    let col =
                        var_get(&mut builder, &vars, &args[2]).expect("Buffer2D col not found");
                    let val =
                        var_get(&mut builder, &vars, &args[3]).expect("Buffer2D val not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_buffer2d_set", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*buf, *row, *col, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "buffer2d_matmul" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs =
                        var_get(&mut builder, &vars, &args[0]).expect("Buffer2D lhs not found");
                    let rhs =
                        var_get(&mut builder, &vars, &args[1]).expect("Buffer2D rhs not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_buffer2d_matmul", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "str_from_obj" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let src = var_get(&mut builder, &vars, &args[0]).expect("Str source not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_str_from_obj", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "repr_from_obj" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Repr source not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_repr_from_obj", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "ascii_from_obj" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Ascii source not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_ascii_from_obj", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dataclass_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let name =
                        var_get(&mut builder, &vars, &args[0]).expect("Dataclass name not found");
                    let fields =
                        var_get(&mut builder, &vars, &args[1]).expect("Dataclass fields not found");
                    let values =
                        var_get(&mut builder, &vars, &args[2]).expect("Dataclass values not found");
                    let flags =
                        var_get(&mut builder, &vars, &args[3]).expect("Dataclass flags not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dataclass_new", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*name, *fields, *values, *flags]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dataclass_get" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Dataclass object not found");
                    let idx =
                        var_get(&mut builder, &vars, &args[1]).expect("Dataclass index not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dataclass_get", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dataclass_set" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Dataclass object not found");
                    let idx =
                        var_get(&mut builder, &vars, &args[1]).expect("Dataclass index not found");
                    let val =
                        var_get(&mut builder, &vars, &args[2]).expect("Dataclass value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dataclass_set", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "dataclass_set_class" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Dataclass object not found");
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_dataclass_set_class", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "lt" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let cmp = builder.ins().fcmp(FloatCC::LessThan, lhs_f, rhs_f);
                        box_bool_value(&mut builder, cmp, &nbc)
                    } else if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let cmp = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp, &nbc)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let cmp = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, rhs_val);
                        let fast_res = box_bool_value(&mut builder, cmp, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_lt", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs, &nbc);
                        let float_block = builder.create_block();
                        let call_block = builder.create_block();
                        builder.set_cold_block(call_block);
                        builder
                            .ins()
                            .brif(both_flt, float_block, &[], call_block, &[]);

                        builder.switch_to_block(float_block);
                        builder.seal_block(float_block);
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let fcmp = builder.ins().fcmp(FloatCC::LessThan, lhs_f, rhs_f);
                        let flt_res = box_bool_value(&mut builder, fcmp, &nbc);
                        jump_block(&mut builder, merge_block, &[flt_res]);

                        builder.switch_to_block(call_block);
                        builder.seal_block(call_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "le" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let cmp = builder.ins().fcmp(FloatCC::LessThanOrEqual, lhs_f, rhs_f);
                        box_bool_value(&mut builder, cmp, &nbc)
                    } else if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let cmp =
                            builder
                                .ins()
                                .icmp(IntCC::SignedLessThanOrEqual, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp, &nbc)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let cmp =
                            builder
                                .ins()
                                .icmp(IntCC::SignedLessThanOrEqual, lhs_val, rhs_val);
                        let fast_res = box_bool_value(&mut builder, cmp, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_le", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs, &nbc);
                        let float_block = builder.create_block();
                        let call_block = builder.create_block();
                        builder.set_cold_block(call_block);
                        builder
                            .ins()
                            .brif(both_flt, float_block, &[], call_block, &[]);

                        builder.switch_to_block(float_block);
                        builder.seal_block(float_block);
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let fcmp = builder.ins().fcmp(FloatCC::LessThanOrEqual, lhs_f, rhs_f);
                        let flt_res = box_bool_value(&mut builder, fcmp, &nbc);
                        jump_block(&mut builder, merge_block, &[flt_res]);

                        builder.switch_to_block(call_block);
                        builder.seal_block(call_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "gt" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let cmp = builder.ins().fcmp(FloatCC::GreaterThan, lhs_f, rhs_f);
                        box_bool_value(&mut builder, cmp, &nbc)
                    } else if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let cmp = builder
                            .ins()
                            .icmp(IntCC::SignedGreaterThan, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp, &nbc)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let cmp = builder
                            .ins()
                            .icmp(IntCC::SignedGreaterThan, lhs_val, rhs_val);
                        let fast_res = box_bool_value(&mut builder, cmp, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_gt", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs, &nbc);
                        let float_block = builder.create_block();
                        let call_block = builder.create_block();
                        builder.set_cold_block(call_block);
                        builder
                            .ins()
                            .brif(both_flt, float_block, &[], call_block, &[]);

                        builder.switch_to_block(float_block);
                        builder.seal_block(float_block);
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let fcmp = builder.ins().fcmp(FloatCC::GreaterThan, lhs_f, rhs_f);
                        let flt_res = box_bool_value(&mut builder, fcmp, &nbc);
                        jump_block(&mut builder, merge_block, &[flt_res]);

                        builder.switch_to_block(call_block);
                        builder.seal_block(call_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "ge" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let cmp = builder
                            .ins()
                            .fcmp(FloatCC::GreaterThanOrEqual, lhs_f, rhs_f);
                        box_bool_value(&mut builder, cmp, &nbc)
                    } else if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let cmp =
                            builder
                                .ins()
                                .icmp(IntCC::SignedGreaterThanOrEqual, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp, &nbc)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let cmp =
                            builder
                                .ins()
                                .icmp(IntCC::SignedGreaterThanOrEqual, lhs_val, rhs_val);
                        let fast_res = box_bool_value(&mut builder, cmp, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_ge", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs, &nbc);
                        let float_block = builder.create_block();
                        let call_block = builder.create_block();
                        builder.set_cold_block(call_block);
                        builder
                            .ins()
                            .brif(both_flt, float_block, &[], call_block, &[]);

                        builder.switch_to_block(float_block);
                        builder.seal_block(float_block);
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let fcmp = builder
                            .ins()
                            .fcmp(FloatCC::GreaterThanOrEqual, lhs_f, rhs_f);
                        let flt_res = box_bool_value(&mut builder, fcmp, &nbc);
                        jump_block(&mut builder, merge_block, &[flt_res]);

                        builder.switch_to_block(call_block);
                        builder.seal_block(call_block);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "eq" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        // Both operands known to be f64 — direct float equality.
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let cmp = builder.ins().fcmp(FloatCC::Equal, lhs_f, rhs_f);
                        box_bool_value(&mut builder, cmp, &nbc)
                    } else if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let cmp = builder.ins().icmp(IntCC::Equal, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp, &nbc)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let cmp = builder.ins().icmp(IntCC::Equal, lhs_val, rhs_val);
                        let fast_res = box_bool_value(&mut builder, cmp, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_eq", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "ne" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        // Both operands known to be f64 — direct float inequality.
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let cmp = builder.ins().fcmp(FloatCC::NotEqual, lhs_f, rhs_f);
                        box_bool_value(&mut builder, cmp, &nbc)
                    } else if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs, &nbc);
                        let rhs_val = unbox_int(&mut builder, *rhs, &nbc);
                        let cmp = builder.ins().icmp(IntCC::NotEqual, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp, &nbc)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs, &nbc);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs, &nbc);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored, &nbc);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        builder
                            .ins()
                            .brif(both_int, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let cmp = builder.ins().icmp(IntCC::NotEqual, lhs_val, rhs_val);
                        let fast_res = box_bool_value(&mut builder, cmp, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_ne", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "string_eq" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    // Use the fast path: pointer-identity check before byte scan.
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_string_eq_fast", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "is" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_is", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "not" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_not", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "neg" | "unary_neg" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Value not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        // -x == 0 - x; overflow only when x == INT_MIN of the
                        // inline payload range.
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_neg", &[types::I64], &[types::I64]);
                        let local_callee =
                            self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let int_val = unbox_int(&mut builder, *val, &nbc);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let negated = builder.ins().isub(zero, int_val);
                        let fits_inline = int_value_fits_inline(&mut builder, negated);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, negated, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*val]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_neg", &[types::I64], &[types::I64]);
                        let local_callee =
                            self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*val]);
                        builder.inst_results(call)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "abs" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Value not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        // abs(x): select(x < 0, -x, x) with overflow check for INT_MIN.
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_abs_builtin", &[types::I64], &[types::I64]);
                        let local_callee =
                            self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let int_val = unbox_int(&mut builder, *val, &nbc);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let is_neg =
                            builder
                                .ins()
                                .icmp(IntCC::SignedLessThan, int_val, zero);
                        let negated = builder.ins().isub(zero, int_val);
                        let abs_val = builder.ins().select(is_neg, negated, int_val);
                        let fits_inline = int_value_fits_inline(&mut builder, abs_val);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, abs_val, &nbc);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let call = builder.ins().call(local_callee, &[*val]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_abs_builtin", &[types::I64], &[types::I64]);
                        let local_callee =
                            self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*val]);
                        builder.inst_results(call)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "invert" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Value not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        // ~x == x ^ -1 for integers; result always fits if input fits
                        // (magnitude changes by at most 1).
                        let int_val = unbox_int(&mut builder, *val, &nbc);
                        let minus_one = builder.ins().iconst(types::I64, -1i64);
                        let inverted = builder.ins().bxor(int_val, minus_one);
                        box_int_value(&mut builder, inverted, &nbc)
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_invert", &[types::I64], &[types::I64]);
                        let local_callee =
                            self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*val]);
                        builder.inst_results(call)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bool" | "cast_bool" | "builtin_bool" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Value not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        // For known ints, bool(x) is simply x != 0.
                        let int_val = unbox_int(&mut builder, *val, &nbc);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let is_nonzero =
                            builder.ins().icmp(IntCC::NotEqual, int_val, zero);
                        box_bool_value(&mut builder, is_nonzero, &nbc)
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_is_truthy", &[types::I64], &[types::I64]);
                        let local_callee =
                            self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*val]);
                        let truthy = builder.inst_results(call)[0];
                        let cond = builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0);
                        box_bool_value(&mut builder, cond, &nbc)
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "and" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let truthy = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_is_truthy", &[types::I64], &[types::I64]);
                    let truthy_ref = self.module.declare_func_in_func(truthy, builder.func);
                    let lhs_call = builder.ins().call(truthy_ref, &[*lhs]);
                    let lhs_val = builder.inst_results(lhs_call)[0];
                    let cond = builder.ins().icmp_imm(IntCC::NotEqual, lhs_val, 0);
                    let res = builder.ins().select(cond, *rhs, *lhs);
                    // The `select` result aliases one of the inputs (same NaN-boxed
                    // bits).  The tracking system will eventually dec_ref the input
                    // name independently of the output name, so we must inc_ref the
                    // result to prevent a use-after-free when the input's refcount
                    // reaches zero before the output is consumed.
                    emit_inc_ref_obj(&mut builder, res, local_inc_ref_obj, &nbc);
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "or" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let truthy = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_is_truthy", &[types::I64], &[types::I64]);
                    let truthy_ref = self.module.declare_func_in_func(truthy, builder.func);
                    let lhs_call = builder.ins().call(truthy_ref, &[*lhs]);
                    let lhs_val = builder.inst_results(lhs_call)[0];
                    let cond = builder.ins().icmp_imm(IntCC::NotEqual, lhs_val, 0);
                    let res = builder.ins().select(cond, *lhs, *rhs);
                    // Same aliasing hazard as `and` — see comment above.
                    emit_inc_ref_obj(&mut builder, res, local_inc_ref_obj, &nbc);
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "contains" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let container =
                        var_get(&mut builder, &vars, &args[0]).expect("Container not found");
                    let item = var_get(&mut builder, &vars, &args[1]).expect("Item not found");
                    let func_name = match op.container_type.as_deref() {
                        Some("set") | Some("frozenset") => "molt_set_contains",
                        Some("dict") => "molt_dict_contains",
                        Some("list") => "molt_list_contains",
                        Some("str") => "molt_str_contains",
                        _ => "molt_contains",
                    };
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function(func_name, Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*container, *item]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "print" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val = if let Some(val) = var_get(&mut builder, &vars, &args[0]) {
                        *val
                    } else {
                        builder.ins().iconst(types::I64, box_none())
                    };

                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_print_obj", &[types::I64], &[]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[val]);
                }
                "print_newline" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_print_newline", &[], &[]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[]);
                }
                "json_parse" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let arg_name = &args[0];
                    if let Some(len) = var_get(&mut builder, &vars, &format!("{}_len", arg_name)) {
                        let ptr = var_get(&mut builder, &vars, &format!("{}_ptr", arg_name))
                            .or_else(|| var_get(&mut builder, &vars, arg_name))
                            .expect("String ptr not found");

                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_json_parse_scalar", &[types::I64, types::I64, types::I64], &[types::I32]);
                        let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*ptr, *len, out_ptr]);
                        let rc = builder.inst_results(call)[0];
                        let ok_block = builder.create_block();
                        let err_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let ok = builder.ins().icmp_imm(IntCC::Equal, rc, 0);
                        brif_block(&mut builder, ok, ok_block, &[], err_block, &[]);

                        builder.switch_to_block(ok_block);
                        builder.seal_block(ok_block);
                        let ok_res =
                            builder
                                .ins()
                                .load(types::I64, MemFlags::trusted(), out_ptr, 0);
                        jump_block(&mut builder, merge_block, &[ok_res]);

                        builder.switch_to_block(err_block);
                        builder.seal_block(err_block);
                        let arg_bits =
                            var_get(&mut builder, &vars, arg_name).expect("String arg not found");
                        let err_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_json_parse_scalar_obj", &[types::I64], &[types::I64]);
                        let err_local = self.module.declare_func_in_func(err_callee, builder.func);
                        let err_call = builder.ins().call(err_local, &[*arg_bits]);
                        let err_res = builder.inst_results(err_call)[0];
                        jump_block(&mut builder, merge_block, &[err_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        let res = builder.block_params(merge_block)[0];
                        if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                    } else {
                        let arg_bits =
                            var_get(&mut builder, &vars, arg_name).expect("String arg not found");
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_json_parse_scalar_obj", &[types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*arg_bits]);
                        let res = builder.inst_results(call)[0];
                        if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                    }
                }
                "msgpack_parse" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let arg_name = &args[0];
                    if let Some(len) = var_get(&mut builder, &vars, &format!("{}_len", arg_name)) {
                        let ptr = var_get(&mut builder, &vars, &format!("{}_ptr", arg_name))
                            .or_else(|| var_get(&mut builder, &vars, arg_name))
                            .expect("Bytes ptr not found");

                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_msgpack_parse_scalar", &[types::I64, types::I64, types::I64], &[types::I32]);
                        let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*ptr, *len, out_ptr]);
                        let rc = builder.inst_results(call)[0];
                        let ok_block = builder.create_block();
                        let err_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let ok = builder.ins().icmp_imm(IntCC::Equal, rc, 0);
                        brif_block(&mut builder, ok, ok_block, &[], err_block, &[]);

                        builder.switch_to_block(ok_block);
                        builder.seal_block(ok_block);
                        let ok_res =
                            builder
                                .ins()
                                .load(types::I64, MemFlags::trusted(), out_ptr, 0);
                        jump_block(&mut builder, merge_block, &[ok_res]);

                        builder.switch_to_block(err_block);
                        builder.seal_block(err_block);
                        let arg_bits =
                            var_get(&mut builder, &vars, arg_name).expect("Bytes arg not found");
                        let err_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_msgpack_parse_scalar_obj", &[types::I64], &[types::I64]);
                        let err_local = self.module.declare_func_in_func(err_callee, builder.func);
                        let err_call = builder.ins().call(err_local, &[*arg_bits]);
                        let err_res = builder.inst_results(err_call)[0];
                        jump_block(&mut builder, merge_block, &[err_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        let res = builder.block_params(merge_block)[0];
                        if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                    } else {
                        let arg_bits =
                            var_get(&mut builder, &vars, arg_name).expect("Bytes arg not found");
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_msgpack_parse_scalar_obj", &[types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*arg_bits]);
                        let res = builder.inst_results(call)[0];
                        if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                    }
                }
                "cbor_parse" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let arg_name = &args[0];
                    if let Some(len) = var_get(&mut builder, &vars, &format!("{}_len", arg_name)) {
                        let ptr = var_get(&mut builder, &vars, &format!("{}_ptr", arg_name))
                            .or_else(|| var_get(&mut builder, &vars, arg_name))
                            .expect("Bytes ptr not found");

                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_cbor_parse_scalar", &[types::I64, types::I64, types::I64], &[types::I32]);
                        let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*ptr, *len, out_ptr]);
                        let rc = builder.inst_results(call)[0];
                        let ok_block = builder.create_block();
                        let err_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let ok = builder.ins().icmp_imm(IntCC::Equal, rc, 0);
                        brif_block(&mut builder, ok, ok_block, &[], err_block, &[]);

                        builder.switch_to_block(ok_block);
                        builder.seal_block(ok_block);
                        let ok_res =
                            builder
                                .ins()
                                .load(types::I64, MemFlags::trusted(), out_ptr, 0);
                        jump_block(&mut builder, merge_block, &[ok_res]);

                        builder.switch_to_block(err_block);
                        builder.seal_block(err_block);
                        let arg_bits =
                            var_get(&mut builder, &vars, arg_name).expect("Bytes arg not found");
                        let err_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_cbor_parse_scalar_obj", &[types::I64], &[types::I64]);
                        let err_local = self.module.declare_func_in_func(err_callee, builder.func);
                        let err_call = builder.ins().call(err_local, &[*arg_bits]);
                        let err_res = builder.inst_results(err_call)[0];
                        jump_block(&mut builder, merge_block, &[err_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        let res = builder.block_params(merge_block)[0];
                        if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                    } else {
                        let arg_bits =
                            var_get(&mut builder, &vars, arg_name).expect("Bytes arg not found");
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_cbor_parse_scalar_obj", &[types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*arg_bits]);
                        let res = builder.inst_results(call)[0];
                        if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                    }
                }
                "block_on" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let task = var_get(&mut builder, &vars, &args[0]).expect("Task not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_block_on", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*task]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "state_switch" => {
                    let self_ptr = builder.block_params(entry_block)[0];
                    // State lives in the cold header (HashMap) — call through
                    // the C API instead of an inline memory load.
                    let get_state_ref = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_obj_get_state",
                        &[types::I64],
                        &[types::I64],
                    );
                    let state_call = builder.ins().call(get_state_ref, &[self_ptr]);
                    let state = builder.inst_results(state_call)[0];
                    let self_bits = box_ptr_value(&mut builder, self_ptr, &nbc);
                    def_var_named(&mut builder, &vars, "self", self_bits);

                    let mut sorted_states: Vec<_> = resume_states.iter().copied().collect();
                    sorted_states.sort();
                    let fallback_block = builder.create_block();
                    let mut switch = Switch::new();
                    for id in sorted_states {
                        let block = state_blocks[&id];
                        switch.set_entry((id as u64) as u128, block);
                        reachable_blocks.insert(block);
                    }
                    reachable_blocks.insert(fallback_block);
                    switch.emit(&mut builder, state, fallback_block);
                    switch_to_block_tracking(&mut builder, fallback_block, &mut is_block_filled);
                }
                "state_transition" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let future = var_get(&mut builder, &vars, &args[0]).expect("Future not found");
                    let future_ptr = unbox_ptr_value(&mut builder, *future, &nbc);
                    let (slot_bits, pending_state_bits) = if args.len() == 2 {
                        (
                            None,
                            *var_get(&mut builder, &vars, &args[1])
                                .expect("Pending state not found"),
                        )
                    } else {
                        (
                            Some(
                                *var_get(&mut builder, &vars, &args[1])
                                    .expect("Await slot not found"),
                            ),
                            *var_get(&mut builder, &vars, &args[2])
                                .expect("Pending state not found"),
                        )
                    };
                    let next_state_id = op.value.unwrap_or(0);
                    let self_bits = *var_get(&mut builder, &vars, "self").expect("Self not found");
                    let self_ptr = unbox_ptr_value(&mut builder, self_bits, &nbc);

                    let pending_state_id = unbox_int(&mut builder, pending_state_bits, &nbc);
                    let set_state_ref = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_obj_set_state",
                        &[types::I64, types::I64],
                        &[],
                    );
                    builder.ins().call(set_state_ref, &[self_ptr, pending_state_id]);

                    let poll_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_future_poll", &[types::I64], &[types::I64]);
                    let local_poll = self.module.declare_func_in_func(poll_callee, builder.func);
                    let poll_call = builder.ins().call(local_poll, &[*future]);
                    let res = builder.inst_results(poll_call)[0];

                    let pending_const = builder.ins().iconst(types::I64, pending_bits());
                    let is_pending = builder.ins().icmp(IntCC::Equal, res, pending_const);

                    let next_block = state_blocks[&next_state_id];
                    let pending_path = builder.create_block();
                    let ready_path = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(pending_path, current_block);
                        builder.insert_block_after(ready_path, pending_path);
                    }
                    reachable_blocks.insert(pending_path);
                    reachable_blocks.insert(ready_path);
                    reachable_blocks.insert(next_block);
                    builder
                        .ins()
                        .brif(is_pending, pending_path, &[], ready_path, &[]);

                    switch_to_block_tracking(&mut builder, pending_path, &mut is_block_filled);
                    builder.seal_block(pending_path);
                    let sleep_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_sleep_register", &[types::I64, types::I64], &[types::I64]);
                    let local_sleep = self.module.declare_func_in_func(sleep_callee, builder.func);
                    builder.ins().call(local_sleep, &[self_ptr, future_ptr]);
                    reachable_blocks.insert(master_return_block);
                    jump_block(&mut builder, master_return_block, &[pending_const]);

                    switch_to_block_tracking(&mut builder, ready_path, &mut is_block_filled);
                    builder.seal_block(ready_path);
                    if let Some(bits) = slot_bits {
                        let offset = unbox_int(&mut builder, bits, &nbc);
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_closure_store", &[types::I64, types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        builder.ins().call(local_callee, &[self_ptr, offset, res]);
                    }
                    let state_val = builder.ins().iconst(types::I64, next_state_id);
                    let set_state_ref2 = import_func_ref(
                        &mut self.module, &mut self.import_ids, &mut builder, &mut import_refs,
                        "molt_obj_set_state", &[types::I64, types::I64], &[],
                    );
                    builder.ins().call(set_state_ref2, &[self_ptr, state_val]);
                    if args.len() <= 1 {
                        if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                    }
                    jump_block(&mut builder, next_block, &[]);

                    switch_to_block_tracking(&mut builder, next_block, &mut is_block_filled);
                }
                "state_yield" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let pair =
                        var_get(&mut builder, &vars, &args[0]).expect("Yield pair not found");
                    let next_state_id = op.value.unwrap_or(0);
                    let self_bits = *var_get(&mut builder, &vars, "self").expect("Self not found");
                    let self_ptr = unbox_ptr_value(&mut builder, self_bits, &nbc);

                    let state_val = builder.ins().iconst(types::I64, next_state_id);
                    let set_state_yield = import_func_ref(
                        &mut self.module, &mut self.import_ids, &mut builder, &mut import_refs,
                        "molt_obj_set_state", &[types::I64, types::I64], &[],
                    );
                    builder.ins().call(set_state_yield, &[self_ptr, state_val]);

                    reachable_blocks.insert(master_return_block);
                    if has_ret {
                        // Suspension returns an owned value to the caller; explicitly
                        // retain it here so downstream cleanup/control-flow lowering cannot
                        // invalidate yielded data before next()/send()/throw() unwraps it.
                        emit_inc_ref_obj(&mut builder, *pair, local_inc_ref_obj, &nbc);
                        jump_block(&mut builder, master_return_block, &[*pair]);
                    } else {
                        jump_block(&mut builder, master_return_block, &[]);
                    }

                    let next_block = state_blocks[&next_state_id];
                    if reachable_blocks.contains(&next_block) {
                        switch_to_block_tracking(&mut builder, next_block, &mut is_block_filled);
                    } else {
                        is_block_filled = true;
                    }
                }
                "chan_send_yield" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let chan = var_get(&mut builder, &vars, &args[0]).expect("Chan not found");
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Val not found");
                    let pending_state_bits =
                        *var_get(&mut builder, &vars, &args[2]).expect("Pending state not found");
                    let next_state_id = op.value.unwrap_or(0);
                    let self_bits = *var_get(&mut builder, &vars, "self").expect("Self not found");
                    let self_ptr = unbox_ptr_value(&mut builder, self_bits, &nbc);

                    let pending_state_id = unbox_int(&mut builder, pending_state_bits, &nbc);
                    let set_state_csend1 = import_func_ref(
                        &mut self.module, &mut self.import_ids, &mut builder, &mut import_refs,
                        "molt_obj_set_state", &[types::I64, types::I64], &[],
                    );
                    builder.ins().call(set_state_csend1, &[self_ptr, pending_state_id]);

                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_chan_send", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*chan, *val]);
                    let res = builder.inst_results(call)[0];

                    let pending_const = builder.ins().iconst(types::I64, pending_bits());
                    let is_pending = builder.ins().icmp(IntCC::Equal, res, pending_const);

                    let next_block = state_blocks[&next_state_id];
                    let ready_path = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(ready_path, current_block);
                    }
                    reachable_blocks.insert(master_return_block);
                    reachable_blocks.insert(ready_path);
                    brif_block(
                        &mut builder,
                        is_pending,
                        master_return_block,
                        &[pending_const],
                        ready_path,
                        &[],
                    );

                    switch_to_block_tracking(&mut builder, ready_path, &mut is_block_filled);
                    let state_val = builder.ins().iconst(types::I64, next_state_id);
                    let set_state_csend2 = import_func_ref(
                        &mut self.module, &mut self.import_ids, &mut builder, &mut import_refs,
                        "molt_obj_set_state", &[types::I64, types::I64], &[],
                    );
                    builder.ins().call(set_state_csend2, &[self_ptr, state_val]);
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                    reachable_blocks.insert(next_block);
                    jump_block(&mut builder, next_block, &[]);

                    if reachable_blocks.contains(&next_block) {
                        switch_to_block_tracking(&mut builder, next_block, &mut is_block_filled);
                    } else {
                        is_block_filled = true;
                    }
                }
                "chan_recv_yield" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let chan = var_get(&mut builder, &vars, &args[0]).expect("Chan not found");
                    let pending_state_bits =
                        *var_get(&mut builder, &vars, &args[1]).expect("Pending state not found");
                    let next_state_id = op.value.unwrap_or(0);
                    let self_bits = *var_get(&mut builder, &vars, "self").expect("Self not found");
                    let self_ptr = unbox_ptr_value(&mut builder, self_bits, &nbc);

                    let pending_state_id = unbox_int(&mut builder, pending_state_bits, &nbc);
                    let set_state_crecv1 = import_func_ref(
                        &mut self.module, &mut self.import_ids, &mut builder, &mut import_refs,
                        "molt_obj_set_state", &[types::I64, types::I64], &[],
                    );
                    builder.ins().call(set_state_crecv1, &[self_ptr, pending_state_id]);

                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_chan_recv", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*chan]);
                    let res = builder.inst_results(call)[0];

                    let pending_const = builder.ins().iconst(types::I64, pending_bits());
                    let is_pending = builder.ins().icmp(IntCC::Equal, res, pending_const);

                    let next_block = state_blocks[&next_state_id];
                    let ready_path = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(ready_path, current_block);
                    }
                    reachable_blocks.insert(master_return_block);
                    reachable_blocks.insert(ready_path);
                    brif_block(
                        &mut builder,
                        is_pending,
                        master_return_block,
                        &[pending_const],
                        ready_path,
                        &[],
                    );

                    switch_to_block_tracking(&mut builder, ready_path, &mut is_block_filled);
                    let state_val = builder.ins().iconst(types::I64, next_state_id);
                    let set_state_crecv2 = import_func_ref(
                        &mut self.module, &mut self.import_ids, &mut builder, &mut import_refs,
                        "molt_obj_set_state", &[types::I64, types::I64], &[],
                    );
                    builder.ins().call(set_state_crecv2, &[self_ptr, state_val]);
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                    reachable_blocks.insert(next_block);
                    jump_block(&mut builder, next_block, &[]);

                    if reachable_blocks.contains(&next_block) {
                        switch_to_block_tracking(&mut builder, next_block, &mut is_block_filled);
                    } else {
                        is_block_filled = true;
                    }
                }
                "chan_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let capacity =
                        var_get(&mut builder, &vars, &args[0]).expect("Capacity not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_chan_new", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*capacity]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "chan_drop" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let chan = var_get(&mut builder, &vars, &args[0]).expect("Chan not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_chan_drop", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*chan]);
                    let _ = builder.inst_results(call)[0];
                }
                "spawn" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let task = var_get(&mut builder, &vars, &args[0]).expect("Task not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_spawn", &[types::I64], &[]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*task]);
                }
                "cancel_token_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let parent =
                        var_get(&mut builder, &vars, &args[0]).expect("Parent token not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_cancel_token_new", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*parent]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "cancel_token_clone" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let token = var_get(&mut builder, &vars, &args[0]).expect("Token not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_cancel_token_clone", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*token]);
                }
                "cancel_token_drop" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let token = var_get(&mut builder, &vars, &args[0]).expect("Token not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_cancel_token_drop", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*token]);
                }
                "cancel_token_cancel" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let token = var_get(&mut builder, &vars, &args[0]).expect("Token not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_cancel_token_cancel", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*token]);
                }
                "future_cancel" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let future = var_get(&mut builder, &vars, &args[0]).expect("Future not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_future_cancel", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*future]);
                }
                "future_cancel_msg" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let future = var_get(&mut builder, &vars, &args[0]).expect("Future not found");
                    let msg =
                        var_get(&mut builder, &vars, &args[1]).expect("Cancel message not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_future_cancel_msg", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*future, *msg]);
                }
                "future_cancel_clear" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let future = var_get(&mut builder, &vars, &args[0]).expect("Future not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_future_cancel_clear", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*future]);
                }
                "promise_new" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_promise_new", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "promise_set_result" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let future = var_get(&mut builder, &vars, &args[0]).expect("Promise not found");
                    let result = var_get(&mut builder, &vars, &args[1]).expect("Result not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_promise_set_result", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*future, *result]);
                }
                "promise_set_exception" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let future = var_get(&mut builder, &vars, &args[0]).expect("Promise not found");
                    let exc = var_get(&mut builder, &vars, &args[1]).expect("Exception not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_promise_set_exception", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*future, *exc]);
                }
                "thread_submit" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let callable =
                        var_get(&mut builder, &vars, &args[0]).expect("Callable not found");
                    let call_args = var_get(&mut builder, &vars, &args[1]).expect("Args not found");
                    let call_kwargs =
                        var_get(&mut builder, &vars, &args[2]).expect("Kwargs not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_thread_submit", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*callable, *call_args, *call_kwargs]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "task_register_token_owned" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let task = var_get(&mut builder, &vars, &args[0]).expect("Task not found");
                    let token = var_get(&mut builder, &vars, &args[1]).expect("Token not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_task_register_token_owned", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*task, *token]);
                }
                "cancel_token_is_cancelled" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let token = var_get(&mut builder, &vars, &args[0]).expect("Token not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_cancel_token_is_cancelled", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*token]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "cancel_token_set_current" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let token = var_get(&mut builder, &vars, &args[0]).expect("Token not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_cancel_token_set_current", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*token]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "cancel_token_get_current" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_cancel_token_get_current", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "cancelled" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_cancelled", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "cancel_current" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_cancel_current", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[]);
                }
                "call_async" => {
                    let Some(poll_func_name) = op.s_value.as_ref() else { continue; };
                    if poll_func_name == "molt_async_sleep" {
                        let arg_names = op.args.as_deref().unwrap_or(&[]);
                        let delay_val = arg_names
                            .first()
                            .map(|name| *var_get(&mut builder, &vars, name).expect("Arg not found"))
                            .unwrap_or_else(|| builder.ins().iconst(types::I64, box_float(0.0)));
                        let result_val = arg_names
                            .get(1)
                            .map(|name| *var_get(&mut builder, &vars, name).expect("Arg not found"))
                            .unwrap_or_else(|| builder.ins().iconst(types::I64, box_none()));
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_async_sleep_new", &[types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[delay_val, result_val]);
                        let res = builder.inst_results(call)[0];
                        let Some(out_name) = op.out else { continue; };
                        def_var_named(&mut builder, &vars, out_name, res);
                    } else {
                        let args = op.args.as_deref();
                        let payload_len = args.map(|vals| vals.len()).unwrap_or(0);
                        let size = builder.ins().iconst(types::I64, (payload_len * 8) as i64);
                        let mut poll_sig = self.module.make_signature();
                        poll_sig.params.push(AbiParam::new(types::I64));
                        poll_sig.returns.push(AbiParam::new(types::I64));
                        let poll_func_id = self
                            .module
                            .declare_function(poll_func_name, Linkage::Import, &poll_sig)
                            .unwrap();
                        let poll_func_ref =
                            self.module.declare_func_in_func(poll_func_id, builder.func);
                        let poll_addr = builder.ins().func_addr(types::I64, poll_func_ref);

                        let task_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_task_new", &[types::I64, types::I64, types::I64], &[types::I64]);
                        let task_local =
                            self.module.declare_func_in_func(task_callee, builder.func);
                        let kind_val = builder.ins().iconst(types::I64, TASK_KIND_FUTURE);
                        let call = builder.ins().call(task_local, &[poll_addr, size, kind_val]);
                        let obj = builder.inst_results(call)[0];
                        let obj_ptr = unbox_ptr_value(&mut builder, obj, &nbc);

                        if let Some(arg_names) = args
                            && !arg_names.is_empty()
                        {
                            for (idx, arg_name) in arg_names.iter().enumerate() {
                                let val =
                                    var_get(&mut builder, &vars, arg_name).expect("Arg not found");
                                builder.ins().store(
                                    MemFlags::trusted(),
                                    *val,
                                    obj_ptr,
                                    (idx * 8) as i32,
                                );
                                emit_inc_ref_obj(&mut builder, *val, local_inc_ref_obj, &nbc);
                            }
                        }
                        let Some(out_name) = op.out else { continue; };
                        def_var_named(&mut builder, &vars, out_name, obj);
                    }
                }
                "builtin_func" => {
                    let Some(func_name) = op.s_value.as_ref() else { continue; };
                    let arity = op.value.unwrap_or(0);
                    let mut func_sig = self.module.make_signature();
                    for _ in 0..arity {
                        func_sig.params.push(AbiParam::new(types::I64));
                    }
                    func_sig.returns.push(AbiParam::new(types::I64));
                    // Reuse existing declaration if the name is already known
                    // (avoids __ov disambiguation when sig differs).
                    let actual_builtin_name = func_name.clone();
                    let func_id = if let Some(cranelift_module::FuncOrDataId::Func(id)) =
                        self.module.get_name(&actual_builtin_name)
                    {
                        id
                    } else {
                        self.module
                            .declare_function(&actual_builtin_name, Linkage::Import, &func_sig)
                            .unwrap_or_else(|e| {
                                panic!(
                                    "builtin_func: failed to declare '{}': {:?}",
                                    actual_builtin_name, e
                                )
                            })
                    };
                    self.declared_func_arities.insert(func_name.clone(), arity as usize);
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let tramp_id = Self::ensure_trampoline(
                        &mut self.module,
                        &mut self.trampoline_ids,
                        &actual_builtin_name,
                        Linkage::Import,
                        TrampolineSpec {
                            arity: arity as usize,
                            has_closure: false,
                            kind: TrampolineKind::Plain,
                            closure_size: 0,
                        },
                    );
                    let tramp_ref = self.module.declare_func_in_func(tramp_id, builder.func);
                    let tramp_addr = builder.ins().func_addr(types::I64, tramp_ref);
                    let arity_val = builder.ins().iconst(types::I64, arity);

                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_func_new_builtin", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[func_addr, tramp_addr, arity_val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "func_new" => {
                    let Some(func_name) = op.s_value.as_ref() else { continue; };
                    let arity = op.value.unwrap_or(0);
                    let kind = if func_name.ends_with("_poll") {
                        task_kinds
                            .get(func_name)
                            .copied()
                            .unwrap_or(TrampolineKind::Plain)
                    } else {
                        TrampolineKind::Plain
                    };
                    let closure_size = if kind == TrampolineKind::Plain {
                        0
                    } else {
                        *task_closure_sizes.get(func_name).unwrap_or(&0)
                    };
                    let mut func_sig = self.module.make_signature();
                    if kind != TrampolineKind::Plain {
                        func_sig.params.push(AbiParam::new(types::I64));
                    } else {
                        for _ in 0..arity {
                            func_sig.params.push(AbiParam::new(types::I64));
                        }
                    }
                    func_sig.returns.push(AbiParam::new(types::I64));
                    self.declared_func_arities.insert(func_name.clone(), func_sig.params.len());
                    // func_new references an existing function. If the symbol is
                    // already declared in this module (same or different sig),
                    // reuse the existing FuncId. This avoids __ov disambiguation
                    // that creates broken stub symbols.
                    let actual_name = func_name.clone();
                    let func_id = if let Some(cranelift_module::FuncOrDataId::Func(id)) =
                        self.module.get_name(&actual_name)
                    {
                        id
                    } else {
                        // Not yet declared — use Import linkage (resolved at link time).
                        self.module
                            .declare_function(&actual_name, Linkage::Import, &func_sig)
                            .unwrap_or_else(|e| {
                                panic!(
                                    "func_new: failed to declare '{}': {:?}",
                                    actual_name, e
                                )
                            })
                    };
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let tramp_id = Self::ensure_trampoline(
                        &mut self.module,
                        &mut self.trampoline_ids,
                        &actual_name,
                        Linkage::Export,
                        TrampolineSpec {
                            arity: arity as usize,
                            has_closure: false,
                            kind,
                            closure_size,
                        },
                    );
                    let tramp_ref = self.module.declare_func_in_func(tramp_id, builder.func);
                    let tramp_addr = builder.ins().func_addr(types::I64, tramp_ref);
                    let arity_val = builder.ins().iconst(types::I64, arity);

                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_func_new", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[func_addr, tramp_addr, arity_val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "func_new_closure" => {
                    let Some(func_name) = op.s_value.as_ref() else { continue; };
                    let arity = op.value.unwrap_or(0);
                    let kind = if func_name.ends_with("_poll") {
                        task_kinds
                            .get(func_name)
                            .copied()
                            .unwrap_or(TrampolineKind::Plain)
                    } else {
                        TrampolineKind::Plain
                    };
                    let closure_size = if kind == TrampolineKind::Plain {
                        0
                    } else {
                        *task_closure_sizes.get(func_name).unwrap_or(&0)
                    };
                    let closure_name = op
                        .args
                        .as_ref()
                        .and_then(|args| args.first())
                        .expect("func_new_closure expects closure arg");
                    let closure_bits =
                        *var_get(&mut builder, &vars, closure_name).expect("closure arg not found");
                    let mut func_sig = self.module.make_signature();
                    if kind != TrampolineKind::Plain {
                        func_sig.params.push(AbiParam::new(types::I64));
                    } else {
                        func_sig.params.push(AbiParam::new(types::I64));
                        for _ in 0..arity {
                            func_sig.params.push(AbiParam::new(types::I64));
                        }
                    }
                    func_sig.returns.push(AbiParam::new(types::I64));
                    self.declared_func_arities.insert(func_name.clone(), func_sig.params.len());
                    let mut actual_closure_name = func_name.clone();
                    // Use Export linkage only when the closure target is
                    // defined in this compilation unit; otherwise Import
                    // (resolved at link time for batched builds).
                    let closure_linkage = if defined_functions.contains(func_name) {
                        Linkage::Export
                    } else {
                        Linkage::Import
                    };
                    let func_id = if let Some(cranelift_module::FuncOrDataId::Func(id)) =
                        self.module.get_name(&actual_closure_name)
                    {
                        id
                    } else {
                        match self
                            .module
                            .declare_function(&actual_closure_name, closure_linkage, &func_sig)
                        {
                            Ok(id) => id,
                            Err(_) => {
                                let mut suffix = 1u32;
                                loop {
                                    actual_closure_name =
                                        format!("{}__ov{}", func_name, suffix);
                                    match self.module.declare_function(
                                        &actual_closure_name,
                                        closure_linkage,
                                        &func_sig,
                                    ) {
                                        Ok(id) => break id,
                                        Err(_) => suffix += 1,
                                    }
                                }
                            }
                        }
                    };
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let tramp_id = Self::ensure_trampoline(
                        &mut self.module,
                        &mut self.trampoline_ids,
                        &actual_closure_name,
                        Linkage::Export,
                        TrampolineSpec {
                            arity: arity as usize,
                            has_closure: true,
                            kind,
                            closure_size,
                        },
                    );
                    let tramp_ref = self.module.declare_func_in_func(tramp_id, builder.func);
                    let tramp_addr = builder.ins().func_addr(types::I64, tramp_ref);
                    let arity_val = builder.ins().iconst(types::I64, arity);

                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_func_new_closure", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[func_addr, tramp_addr, arity_val, closure_bits],
                    );
                    let res = builder.inst_results(call)[0];
                    // Track closure function object for direct calls
                    if let Some(out_name) = op.out.as_ref() {
                        local_closure_envs.insert(func_name.clone(), out_name.clone());
                    }
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "code_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let filename_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("filename not found");
                    let name_bits = var_get(&mut builder, &vars, &args[1]).expect("name not found");
                    let firstlineno_bits =
                        var_get(&mut builder, &vars, &args[2]).expect("firstlineno not found");
                    let linetable_bits =
                        var_get(&mut builder, &vars, &args[3]).expect("linetable not found");
                    let varnames_bits =
                        var_get(&mut builder, &vars, &args[4]).expect("varnames not found");
                    let argcount_bits =
                        var_get(&mut builder, &vars, &args[5]).expect("argcount not found");
                    let posonlyargcount_bits =
                        var_get(&mut builder, &vars, &args[6]).expect("posonly not found");
                    let kwonlyargcount_bits =
                        var_get(&mut builder, &vars, &args[7]).expect("kwonly not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_code_new", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[
                            *filename_bits,
                            *name_bits,
                            *firstlineno_bits,
                            *linetable_bits,
                            *varnames_bits,
                            *argcount_bits,
                            *posonlyargcount_bits,
                            *kwonlyargcount_bits,
                        ],
                    );
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "code_slot_set" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let code_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("code bits not found");
                    let code_id = op.value.unwrap_or(0);
                    let code_id_val = builder.ins().iconst(types::I64, code_id);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_code_slot_set", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder.ins().call(local_callee, &[code_id_val, *code_bits]);
                }
                "fn_ptr_code_set" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let code_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("code bits not found");
                    let func_name = op.s_value.as_ref().expect("fn_ptr_code_set expects symbol");
                    let func_id = if let Some(cranelift_module::FuncOrDataId::Func(id)) =
                        self.module.get_name(func_name)
                    {
                        id
                    } else {
                        let mut func_sig = self.module.make_signature();
                        let arity = op.value.unwrap_or(0);
                        if arity > 0 {
                            for _ in 0..arity {
                                func_sig.params.push(AbiParam::new(types::I64));
                            }
                        } else if func_name.ends_with("_poll") {
                            func_sig.params.push(AbiParam::new(types::I64));
                        }
                        func_sig.returns.push(AbiParam::new(types::I64));
                        // Use Export only when the target is defined in this
                        // compilation unit; otherwise Import (resolved at link
                        // time).  Using unconditional Export here was causing
                        // "Export must be defined" panics when dead function
                        // elimination removed the target.
                        let linkage = if defined_functions.contains(func_name) {
                            Linkage::Export
                        } else {
                            Linkage::Import
                        };
                        self.module
                            .declare_function(func_name, linkage, &func_sig)
                            .unwrap()
                    };
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_fn_ptr_code_set", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder.ins().call(local_callee, &[func_addr, *code_bits]);
                }
                "asyncgen_locals_register" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let names_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("names tuple not found");
                    let offsets_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("offsets tuple not found");
                    let func_name = op
                        .s_value
                        .as_ref()
                        .expect("asyncgen_locals_register expects symbol");
                    let func_id = if let Some(cranelift_module::FuncOrDataId::Func(id)) =
                        self.module.get_name(func_name)
                    {
                        id
                    } else {
                        let mut func_sig = self.module.make_signature();
                        let arity = op.value.unwrap_or(0);
                        if arity > 0 {
                            for _ in 0..arity {
                                func_sig.params.push(AbiParam::new(types::I64));
                            }
                        } else if func_name.ends_with("_poll") {
                            func_sig.params.push(AbiParam::new(types::I64));
                        }
                        func_sig.returns.push(AbiParam::new(types::I64));
                        let linkage = if defined_functions.contains(func_name) {
                            Linkage::Export
                        } else {
                            Linkage::Import
                        };
                        self.module
                            .declare_function(func_name, linkage, &func_sig)
                            .unwrap()
                    };
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_asyncgen_locals_register", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder
                        .ins()
                        .call(local_callee, &[func_addr, *names_bits, *offsets_bits]);
                }
                "gen_locals_register" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let names_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("names tuple not found");
                    let offsets_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("offsets tuple not found");
                    let func_name = op
                        .s_value
                        .as_ref()
                        .expect("gen_locals_register expects symbol");
                    // Build the signature from the op's declared arity.
                    let mut func_sig = self.module.make_signature();
                    let arity = op.value.unwrap_or(0);
                    if arity > 0 {
                        for _ in 0..arity {
                            func_sig.params.push(AbiParam::new(types::I64));
                        }
                    } else if func_name.ends_with("_poll") {
                        func_sig.params.push(AbiParam::new(types::I64));
                    }
                    func_sig.returns.push(AbiParam::new(types::I64));
                    // The function may have been declared by func_new with a
                    // different (trampoline) signature.  Reuse the existing
                    // FuncId when available; if signatures conflict, the
                    // linker resolves the difference via the trampoline.
                    let func_id = if let Some(cranelift_module::FuncOrDataId::Func(id)) =
                        self.module.get_name(func_name)
                    {
                        id
                    } else {
                        let linkage = if defined_functions.contains(func_name) {
                            Linkage::Export
                        } else {
                            Linkage::Import
                        };
                        self.module
                            .declare_function(func_name, linkage, &func_sig)
                            .unwrap_or_else(|e| {
                                panic!(
                                    "gen_locals_register: failed to declare '{}': {:?}",
                                    func_name, e
                                )
                            })
                    };
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_gen_locals_register", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder
                        .ins()
                        .call(local_callee, &[func_addr, *names_bits, *offsets_bits]);
                }
                "code_slots_init" => {
                    let count = op.value.unwrap_or(0);
                    let count_val = builder.ins().iconst(types::I64, count);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_code_slots_init", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder.ins().call(local_callee, &[count_val]);
                }
                "trace_enter_slot" => {
                    if emit_traces {
                        let code_id = op.value.unwrap_or(0);
                        let code_id_val = builder.ins().iconst(types::I64, code_id);
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_trace_enter_slot", &[types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let _ = builder.ins().call(local_callee, &[code_id_val]);
                    }
                }
                "trace_exit" => {
                    if emit_traces {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_trace_exit", &[], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let _ = builder.ins().call(local_callee, &[]);
                    }
                }
                "frame_locals_set" => {
                    let arg_names = op.args.as_deref().unwrap_or(&[]);
                    let dict_bits = arg_names
                        .first()
                        .map(|name| *var_get(&mut builder, &vars, name).expect("Arg not found"))
                        .unwrap_or_else(|| builder.ins().iconst(types::I64, 0));
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_frame_locals_set", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder.ins().call(local_callee, &[dict_bits]);
                }
                "line" => {
                    let line = op.value.unwrap_or(0);
                    let line_val = builder.ins().iconst(types::I64, line);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_trace_set_line", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder.ins().call(local_callee, &[line_val]);
                    if !is_block_filled && let Some(block) = builder.current_block() {
                        if let Some(names) = block_tracked_obj.get_mut(&block) {
                            let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                            for name in cleanup {
                                // Use entry_vars (definition-time Value) for dec_ref,
                                // not var_get (current SSA Value). If the variable was
                                // redefined, var_get returns the WRONG object.
                                let val = entry_vars.get(&name).copied()
                                    .or_else(|| var_get(&mut builder, &vars, &name).map(|v| *v));
                                let Some(val) = val else { continue; };
                                builder.ins().call(local_dec_ref_obj, &[val]);
                                // Remove from entry_vars so exception-handler
                                // and function-return cleanup paths do not
                                // dec-ref this already-freed variable again.
                                entry_vars.remove(&name);
                            }
                        }
                        if let Some(names) = block_tracked_ptr.get_mut(&block) {
                            let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                            for name in cleanup {
                                let val = entry_vars.get(&name).copied()
                                    .or_else(|| var_get(&mut builder, &vars, &name).map(|v| *v));
                                let Some(val) = val else { continue; };
                                builder.ins().call(local_dec_ref_obj, &[val]);
                                entry_vars.remove(&name);
                            }
                        }
                    }
                }
                "missing" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_missing", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "function_closure_bits" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let func_bits = var_get(&mut builder, &vars, &args[0]).expect("Func not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_function_closure_bits", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*func_bits]);
                    let res = builder.inst_results(call)[0];
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj, &nbc);
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bound_method_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let func_bits = var_get(&mut builder, &vars, &args[0]).expect("Func not found");
                    let self_bits = var_get(&mut builder, &vars, &args[1]).expect("Self not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bound_method_new", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*func_bits, *self_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "call" => {
                    let Some(target_name) = op.s_value.as_ref() else { continue; };
                    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let mut args = Vec::new();
                    for name in args_names {
                        args.push(*var_get(&mut builder, &vars, name).expect("Arg not found"));
                    }

                    // Collect arg values that are dead after this call. We explicitly avoid
                    // decrementing function parameters here: parameters are treated as borrowed
                    // by this backend (caller owns), so only non-param temporaries should be
                    // released at the call site.
                    let mut arg_cleanup = Vec::new();
                    let mut arg_cleanup_names = BTreeSet::new();
                    for (name, value) in args_names.iter().zip(args.iter()) {
                        if param_name_set.contains(name.as_str()) {
                            continue;
                        }
                        let last = last_use.get(name).copied().unwrap_or(op_idx);
                        if last <= op_idx {
                            arg_cleanup.push(*value);
                            arg_cleanup_names.insert(name.clone());
                        }
                    }

                    // `call` lowers to a multi-block control-flow sequence (recursion guard +
                    // call block + fail block + merge block). If the call happens in a non-entry
                    // block, any temporaries tracked on the current block would otherwise be
                    // orphaned when we terminate the block with the guard brif. Drain the
                    // current block's tracked sets here, but emit the actual decrefs *after* the
                    // call (or on the guard-fail path) so arguments remain alive during the call.
                    let origin_block = builder
                        .current_block()
                        .expect("call requires an active block");
                    let mut origin_obj_live =
                        block_tracked_obj.remove(&origin_block).unwrap_or_default();
                    let origin_obj_cleanup =
                        drain_cleanup_tracked(&mut origin_obj_live, &last_use, op_idx, None);
                    let mut origin_ptr_live =
                        block_tracked_ptr.remove(&origin_block).unwrap_or_default();
                    let origin_ptr_cleanup =
                        drain_cleanup_tracked(&mut origin_ptr_live, &last_use, op_idx, None);
                    if std::env::var("MOLT_DEBUG_CALL_CLEANUP").as_deref() == Ok("1")
                        && std::env::var("MOLT_DEBUG_FUNC_FILTER")
                            .ok()
                            .map_or(true, |f| func_ir.name.contains(&f))
                    {
                        let obj_names: Vec<&str> =
                            origin_obj_cleanup.iter().map(|t| t.as_str()).collect();
                        let ptr_names: Vec<&str> =
                            origin_ptr_cleanup.iter().map(|t| t.as_str()).collect();
                        eprintln!(
                            "debug call cleanup func={} op_idx={} origin_block={:?} obj_cleanup={} ptr_cleanup={}",
                            func_ir.name,
                            op_idx,
                            origin_block,
                            obj_names.len(),
                            ptr_names.len(),
                        );
                        if !obj_names.is_empty() {
                            eprintln!("debug call cleanup obj_names={:?}", obj_names);
                        }
                        if !ptr_names.is_empty() {
                            eprintln!("debug call cleanup ptr_names={:?}", ptr_names);
                        }
                    }

                    // For direct calls to closures, extract env from function object
                    if closure_functions.contains(target_name.as_str())
                        && let Some(func_obj_var) = local_closure_envs.get(target_name.as_str())
                    {
                        let func_obj_bits = *var_get(&mut builder, &vars, func_obj_var)
                            .expect("Closure func obj not found for direct call");
                        let extract_local = import_func_ref(
                            &mut self.module,
                            &mut self.import_ids,
                            &mut builder,
                            &mut import_refs,
                            "molt_function_closure_bits",
                            &[types::I64],
                            &[types::I64],
                        );
                        let extract_call = builder.ins().call(extract_local, &[func_obj_bits]);
                        let env_bits = builder.inst_results(extract_call)[0];
                        args.insert(0, env_bits);
                    }
                    // Declare the target function.
                    // Use the previously-declared arity if available, so the
                    // Cranelift signature matches the definition even when the
                    // call site passes a different number of arguments (e.g.
                    // expanded keyword arguments).
                    let sig_arity = self
                        .declared_func_arities
                        .get(target_name.as_str())
                        .copied()
                        .unwrap_or(args.len());
                    let mut target_sig = self.module.make_signature();
                    for _ in 0..sig_arity {
                        target_sig.params.push(AbiParam::new(types::I64));
                    }
                    target_sig.returns.push(AbiParam::new(types::I64));
                    let linkage = if defined_functions.contains(target_name) {
                        Linkage::Export
                    } else {
                        Linkage::Import
                    };
                    let callee = match self
                        .module
                        .declare_function(target_name, linkage, &target_sig)
                    {
                        Ok(id) => id,
                        Err(_) => {
                            // Function was already declared with a different signature
                            // (e.g., @typing.overload stubs vs real implementation).
                            // Look up the existing declaration instead of panicking.
                            self.module
                                .declare_function(target_name, linkage, &{
                                    let mut s = self.module.make_signature();
                                    // Use args.len() as fallback — the runtime dispatch
                                    // (molt_guarded_call) handles arity mismatch.
                                    for _ in 0..args.len() {
                                        s.params.push(AbiParam::new(types::I64));
                                    }
                                    s.returns.push(AbiParam::new(types::I64));
                                    s
                                })
                                .unwrap_or_else(|_| {
                                    // Both arities failed — use the existing func ID
                                    // by looking it up through get_name
                                    self.module.get_name(target_name)
                                        .and_then(|name_id| {
                                            if let cranelift_module::FuncOrDataId::Func(fid) = name_id {
                                                Some(fid)
                                            } else {
                                                None
                                            }
                                        })
                                        .expect("function must have been declared")
                                })
                        }
                    };
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);

                    // --- Fast path: direct call for known defined non-closure functions ---
                    // When the target is a defined function in this module (not a closure),
                    // emit a direct Cranelift call with a lightweight recursion guard.
                    // This avoids: arg spill/reload, match-on-arity dispatch, indirect call.
                    let use_direct_call = defined_functions.contains(target_name)
                        && !closure_functions.contains(target_name.as_str())
                        && args.len() == sig_arity
                        && !emit_traces;

                    if std::env::var("MOLT_DEBUG_DIRECT_CALL").is_ok() {
                        eprintln!(
                            "call {} -> direct={} (defined={} closure={} arity_match={} traces={})",
                            target_name,
                            use_direct_call,
                            defined_functions.contains(target_name),
                            closure_functions.contains(target_name.as_str()),
                            args.len() == sig_arity,
                            emit_traces,
                        );
                    }

                    let res = if use_direct_call && leaf_functions.contains(target_name) {
                        // Leaf function: no user-level calls inside, so it
                        // cannot recurse.  Skip the recursion guard entirely
                        // (saves 2 atomic ops + 2 extern-C calls per call).
                        let direct_call = builder.ins().call(local_callee, &args);
                        builder.inst_results(direct_call)[0]
                    } else if use_direct_call {
                        // Lightweight recursion guard using global atomics
                        // (no TLS on the hot path). The data-symbol inline
                        // approach was reverted because Cranelift global_value
                        // addresses caused segfaults on some programs.
                        let enter_ref = import_func_ref(
                            &mut self.module,
                            &mut self.import_ids,
                            &mut builder,
                            &mut import_refs,
                            "molt_recursion_enter_fast",
                            &[],
                            &[types::I64],
                        );
                        let enter_call = builder.ins().call(enter_ref, &[]);
                        let guard_ok = builder.inst_results(enter_call)[0];

                        // Branch on recursion guard result.
                        let call_block = builder.create_block();
                        let error_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let zero = builder.ins().iconst(types::I64, 0);
                        let is_ok = builder.ins().icmp(IntCC::NotEqual, guard_ok, zero);
                        brif_block(&mut builder, is_ok, call_block, &[], error_block, &[]);

                        // Error block: recursion limit exceeded (cold path).
                        builder.switch_to_block(error_block);
                        let raise_ref = import_func_ref(
                            &mut self.module,
                            &mut self.import_ids,
                            &mut builder,
                            &mut import_refs,
                            "molt_raise_recursion_error",
                            &[],
                            &[types::I64],
                        );
                        let raise_call = builder.ins().call(raise_ref, &[]);
                        let err_val = builder.inst_results(raise_call)[0];
                        jump_block(&mut builder, merge_block, &[err_val]);

                        // Call block: direct call to the target function.
                        builder.switch_to_block(call_block);
                        let direct_call = builder.ins().call(local_callee, &args);
                        let call_res = builder.inst_results(direct_call)[0];

                        // Exit recursion guard.
                        let exit_ref = import_func_ref(
                            &mut self.module,
                            &mut self.import_ids,
                            &mut builder,
                            &mut import_refs,
                            "molt_recursion_exit_fast",
                            &[],
                            &[],
                        );
                        builder.ins().call(exit_ref, &[]);
                        jump_block(&mut builder, merge_block, &[call_res]);

                        builder.switch_to_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        // --- Outlined guarded call via molt_guarded_call ---
                        // Fallback for imported functions, closures, arity mismatches,
                        // or when tracing is enabled.
                        let fn_ptr_val = builder.ins().func_addr(types::I64, local_callee);

                        // Spill args to a stack slot for the outlined helper.
                        let nargs_count = args.len();
                        let slot_size = std::cmp::max(nargs_count, 1) * 8;
                        let args_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            slot_size as u32,
                            3, // align_shift: 2^3 = 8-byte alignment
                        ));
                        for (i, arg) in args.iter().enumerate() {
                            builder.ins().stack_store(*arg, args_slot, (i * 8) as i32);
                        }
                        let args_ptr_val = builder.ins().stack_addr(types::I64, args_slot, 0);
                        let nargs_val = builder.ins().iconst(types::I64, nargs_count as i64);
                        let code_id_val = if emit_traces {
                            builder.ins().iconst(types::I64, op.value.unwrap_or(0))
                        } else {
                            builder.ins().iconst(types::I64, -1i64)
                        };

                        // Declare and call molt_guarded_call.
                        let gc_local = import_func_ref(
                            &mut self.module,
                            &mut self.import_ids,
                            &mut builder,
                            &mut import_refs,
                            "molt_guarded_call",
                            &[types::I64, types::I64, types::I64, types::I64],
                            &[types::I64],
                        );
                        let gc_call = builder.ins().call(
                            gc_local,
                            &[fn_ptr_val, args_ptr_val, nargs_val, code_id_val],
                        );
                        builder.inst_results(gc_call)[0]
                    };

                    // Tracked-value cleanup (stays inline — varies per site).
                    // Re-attach surviving tracked values to the current block.
                    if let Some(cur_block) = builder.current_block() {
                        if !origin_obj_live.is_empty() {
                            extend_unique_tracked(
                                block_tracked_obj.entry(cur_block).or_default(),
                                origin_obj_live,
                            );
                        }
                        if !origin_ptr_live.is_empty() {
                            extend_unique_tracked(
                                block_tracked_ptr.entry(cur_block).or_default(),
                                origin_ptr_live,
                            );
                        }
                    }
                    for name in &origin_obj_cleanup {
                        if arg_cleanup_names.contains(name) {
                            continue;
                        }
                        // Use entry_vars (definition-time Value) for dec_ref,
                        // not var_get (current SSA Value). If the variable was
                        // redefined, var_get returns the WRONG object.
                        let val = entry_vars.get(name).copied()
                            .or_else(|| var_get(&mut builder, &vars, name).map(|v| *v));
                        let Some(val) = val else { continue; };
                        builder.ins().call(local_dec_ref_obj, &[val]);
                    }
                    for name in &origin_ptr_cleanup {
                        let val = entry_vars.get(name).copied()
                            .or_else(|| var_get(&mut builder, &vars, name).map(|v| *v));
                        let Some(val) = val else { continue; };
                        builder.ins().call(local_dec_ref_obj, &[val]);
                    }
                    for val in &arg_cleanup {
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                    // Remove cleaned-up names from entry-tracked lists so the
                    // function-return cleanup does not dec-ref them a second
                    // time (the `call` op changes blocks, so the normal
                    // entry-tracked drain no longer runs for these variables).
                    if !arg_cleanup_names.is_empty() {
                        tracked_obj_vars.retain(|n| !arg_cleanup_names.contains(n));
                        tracked_vars.retain(|n| !arg_cleanup_names.contains(n));
                        for name in &arg_cleanup_names {
                            tracked_obj_vars_set.remove(name);
                            tracked_vars_set.remove(name);
                            entry_vars.remove(name);
                        }
                    }
                    for name in &origin_obj_cleanup {
                        if !arg_cleanup_names.contains(name) {
                            tracked_obj_vars.retain(|n| n != name);
                            tracked_obj_vars_set.remove(name);
                            entry_vars.remove(name);
                        }
                    }
                    for name in &origin_ptr_cleanup {
                        tracked_vars.retain(|n| n != name);
                        tracked_vars_set.remove(name);
                        entry_vars.remove(name);
                    }
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "call_internal" => {
                    let Some(target_name) = op.s_value.as_ref() else { continue; };
                    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let mut args = Vec::new();
                    for name in args_names {
                        args.push(*var_get(&mut builder, &vars, name).expect("Arg not found"));
                    }

                    // For direct calls to closures, extract env from function object
                    if closure_functions.contains(target_name.as_str())
                        && let Some(func_obj_var) = local_closure_envs.get(target_name.as_str())
                    {
                        let func_obj_bits = *var_get(&mut builder, &vars, func_obj_var)
                            .expect("Closure func obj not found for direct call");
                        let extract_local = import_func_ref(
                            &mut self.module,
                            &mut self.import_ids,
                            &mut builder,
                            &mut import_refs,
                            "molt_function_closure_bits",
                            &[types::I64],
                            &[types::I64],
                        );
                        let extract_call = builder.ins().call(extract_local, &[func_obj_bits]);
                        let env_bits = builder.inst_results(extract_call)[0];
                        args.insert(0, env_bits);
                    }
                    let mut sig = self.module.make_signature();
                    for _ in 0..args.len() {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let linkage = if defined_functions.contains(target_name) {
                        Linkage::Export
                    } else {
                        Linkage::Import
                    };

                    let callee = self
                        .module
                        .declare_function(target_name, linkage, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &args);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "inc_ref" | "borrow" => {
                    if !rc_skip_inc.contains(&op_idx) {
                        let args_names = op.args.as_ref().expect("inc_ref/borrow args missing");
                        let src_name = args_names
                            .first()
                            .expect("inc_ref/borrow requires one source arg");
                        let src = *var_get(&mut builder, &vars, src_name)
                            .expect("inc_ref/borrow source not found");
                        emit_inc_ref_obj(&mut builder, src, local_inc_ref_obj, &nbc);
                        if let Some(out_name) = op.out.as_ref()
                            && out_name != "none"
                        {
                            def_var_named(&mut builder, &vars, out_name.clone(), src);
                        }
                    } else if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        // RC coalesced: still define the output variable as an
                        // alias of the input so downstream ops can read it.
                        let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                        let src_name = args_names.first().unwrap();
                        let src = *var_get(&mut builder, &vars, src_name)
                            .expect("inc_ref/borrow source not found (coalesced)");
                        def_var_named(&mut builder, &vars, out_name.clone(), src);
                    }
                }
                "dec_ref" | "release" => {
                    let args_names =
                        op.args.as_ref().expect("dec_ref/release args missing");
                    let src_name = args_names
                        .first()
                        .expect("dec_ref/release requires one source arg");
                    // Skip dec_ref for stack-allocated tuples — their memory
                    // is freed automatically when the stack frame unwinds.
                    let is_stack_tuple = stack_tuples.contains_key(src_name.as_str());
                    if is_stack_tuple || rc_skip_inc.contains(&op_idx) {
                        // No runtime call needed.  Still define the output
                        // variable so downstream SSA reads succeed.
                        if let Some(out_name) = op.out.as_ref()
                            && out_name != "none"
                        {
                            let none_bits = builder.ins().iconst(types::I64, box_none());
                            def_var_named(&mut builder, &vars, out_name.clone(), none_bits);
                        }
                    } else {
                        let src = *var_get(&mut builder, &vars, src_name)
                            .expect("dec_ref/release source not found");
                        builder.ins().call(local_dec_ref_obj, &[src]);
                        if let Some(out_name) = op.out.as_ref()
                            && out_name != "none"
                        {
                            let none_bits = builder.ins().iconst(types::I64, box_none());
                            def_var_named(&mut builder, &vars, out_name.clone(), none_bits);
                        }
                    }
                }
                "box" | "unbox" | "cast" | "widen" => {
                    let args_names = op.args.as_ref().expect("conversion args missing");
                    let src_name = args_names
                        .first()
                        .expect("conversion op requires one source arg");
                    let src = *var_get(&mut builder, &vars, src_name)
                        .expect("conversion source not found");
                    if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        // Output aliases input bits — inc_ref to prevent
                        // use-after-free when the input name is dec_ref'd
                        // independently by tracking/check_exception cleanup.
                        emit_inc_ref_obj(&mut builder, src, local_inc_ref_obj, &nbc);
                        def_var_named(&mut builder, &vars, out_name.clone(), src);
                    }
                }
                "identity_alias" => {
                    let args_names = op.args.as_ref().expect("identity_alias args missing");
                    let src_name = args_names
                        .first()
                        .expect("identity_alias requires one source arg");
                    let src = *var_get(&mut builder, &vars, src_name)
                        .expect("identity_alias source not found");
                    if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        // Same aliasing hazard as box/unbox/cast/widen above.
                        emit_inc_ref_obj(&mut builder, src, local_inc_ref_obj, &nbc);
                        def_var_named(&mut builder, &vars, out_name.clone(), src);
                    }
                }
                "call_guarded" => {
                    let Some(target_name) = op.s_value.as_ref() else { continue; };
                    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let callee_bits =
                        var_get(&mut builder, &vars, &args_names[0]).expect("Callee not found");
                    let mut args = Vec::new();
                    for name in &args_names[1..] {
                        args.push(*var_get(&mut builder, &vars, name).expect("Arg not found"));
                    }

                    // For direct calls to closures, extract env from function object
                    if closure_functions.contains(target_name.as_str())
                        && let Some(func_obj_var) = local_closure_envs.get(target_name.as_str())
                    {
                        let func_obj_bits = *var_get(&mut builder, &vars, func_obj_var)
                            .expect("Closure func obj not found for direct call");
                        let extract_fn = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_function_closure_bits", &[types::I64], &[types::I64]);
                        let extract_local =
                            self.module.declare_func_in_func(extract_fn, builder.func);
                        let extract_call = builder.ins().call(extract_local, &[func_obj_bits]);
                        let env_bits = builder.inst_results(extract_call)[0];
                        args.insert(0, env_bits);
                    }
                    // Use the previously-declared arity if available so the
                    // Cranelift signature matches the definition even when the
                    // call site passes a different number of arguments.
                    let sig_arity = self
                        .declared_func_arities
                        .get(target_name.as_str())
                        .copied()
                        .unwrap_or(args.len());
                    let mut sig = self.module.make_signature();
                    for _ in 0..sig_arity {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let linkage = if defined_functions.contains(target_name) {
                        Linkage::Export
                    } else {
                        Linkage::Import
                    };

                    let callee = match self
                        .module
                        .declare_function(target_name, linkage, &sig)
                    {
                        Ok(id) => id,
                        Err(_) => {
                            // Signature mismatch — reuse existing declaration
                            self.module.get_name(target_name)
                                .and_then(|name_id| {
                                    if let cranelift_module::FuncOrDataId::Func(fid) = name_id {
                                        Some(fid)
                                    } else {
                                        None
                                    }
                                })
                                .expect("function must have been declared")
                        }
                    };
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let expected_addr = builder.ins().func_addr(types::I64, local_callee);

                    let is_func_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_is_function_obj",
                        &[types::I64],
                        &[types::I64],
                    );
                    let truthy_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_is_truthy",
                        &[types::I64],
                        &[types::I64],
                    );
                    let guard_enter_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_recursion_guard_enter",
                        &[],
                        &[types::I64],
                    );
                    let guard_exit_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_recursion_guard_exit",
                        &[],
                        &[],
                    );
                    let trace_enter_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_trace_enter",
                        &[types::I64],
                        &[types::I64],
                    );
                    let trace_exit_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_trace_exit",
                        &[],
                        &[types::I64],
                    );
                    let is_func_call = builder.ins().call(is_func_local, &[*callee_bits]);
                    let is_func_bits = builder.inst_results(is_func_call)[0];
                    let truthy_call = builder.ins().call(truthy_local, &[is_func_bits]);
                    let truthy_bits = builder.inst_results(truthy_call)[0];
                    let is_func_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy_bits, 0);

                    let resolve_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_handle_resolve",
                        &[types::I64],
                        &[types::I64],
                    );
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);

                    let func_block = builder.create_block();
                    let fallback_block = builder.create_block();
                    builder
                        .ins()
                        .brif(is_func_bool, func_block, &[], fallback_block, &[]);

                    builder.switch_to_block(fallback_block);
                    builder.seal_block(fallback_block);
                    let callargs_new_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_callargs_new",
                        &[types::I64, types::I64],
                        &[types::I64],
                    );
                    let pos_capacity = builder.ins().iconst(types::I64, args.len() as i64);
                    let kw_capacity = builder.ins().iconst(types::I64, 0);
                    let callargs_call = builder
                        .ins()
                        .call(callargs_new_local, &[pos_capacity, kw_capacity]);
                    let callargs_ptr = builder.inst_results(callargs_call)[0];
                    let callargs_push_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_callargs_push_pos",
                        &[types::I64, types::I64],
                        &[types::I64],
                    );
                    for arg in &args {
                        builder
                            .ins()
                            .call(callargs_push_local, &[callargs_ptr, *arg]);
                    }
                    let call_bind_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_call_bind_ic",
                        &[types::I64, types::I64, types::I64],
                        &[types::I64],
                    );
                    let site_bits = builder.ins().iconst(
                        types::I64,
                        box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            "call_guarded",
                        )),
                    );
                    let fallback_call = builder
                        .ins()
                        .call(call_bind_local, &[site_bits, *callee_bits, callargs_ptr]);
                    let fallback_res = builder.inst_results(fallback_call)[0];
                    jump_block(&mut builder, merge_block, &[fallback_res]);

                    builder.switch_to_block(func_block);
                    builder.seal_block(func_block);
                    let resolve_call = builder.ins().call(resolve_local, &[*callee_bits]);
                    let func_ptr = builder.inst_results(resolve_call)[0];
                    let fn_ptr = builder
                        .ins()
                        .load(types::I64, MemFlags::trusted(), func_ptr, 0);
                    let matches = builder.ins().icmp(IntCC::Equal, fn_ptr, expected_addr);
                    let then_block = builder.create_block();
                    let else_block = builder.create_block();
                    builder
                        .ins()
                        .brif(matches, then_block, &[], else_block, &[]);

                    builder.switch_to_block(then_block);
                    builder.seal_block(then_block);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let then_call_block = builder.create_block();
                    let then_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, then_call_block, &[], then_fail_block, &[]);

                    builder.switch_to_block(then_call_block);
                    builder.seal_block(then_call_block);
                    if emit_traces {
                        let _ = builder.ins().call(trace_enter_local, &[*callee_bits]);
                    }
                    let direct_call = builder.ins().call(local_callee, &args);
                    let direct_res = builder.inst_results(direct_call)[0];
                    if emit_traces {
                        let _ = builder.ins().call(trace_exit_local, &[]);
                    }
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    jump_block(&mut builder, merge_block, &[direct_res]);

                    builder.switch_to_block(then_fail_block);
                    builder.seal_block(then_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    jump_block(&mut builder, merge_block, &[none_bits]);

                    builder.switch_to_block(else_block);
                    builder.seal_block(else_block);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let else_call_block = builder.create_block();
                    let else_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, else_call_block, &[], else_fail_block, &[]);

                    builder.switch_to_block(else_call_block);
                    builder.seal_block(else_call_block);
                    if emit_traces {
                        let _ = builder.ins().call(trace_enter_local, &[*callee_bits]);
                    }
                    let sig_ref = builder.import_signature(sig);
                    let fallback_call = builder.ins().call_indirect(sig_ref, fn_ptr, &args);
                    let fallback_res = builder.inst_results(fallback_call)[0];
                    if emit_traces {
                        let _ = builder.ins().call(trace_exit_local, &[]);
                    }
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    jump_block(&mut builder, merge_block, &[fallback_res]);

                    builder.switch_to_block(else_fail_block);
                    builder.seal_block(else_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    jump_block(&mut builder, merge_block, &[none_bits]);

                    builder.switch_to_block(merge_block);
                    builder.seal_block(merge_block);
                    let res = builder.block_params(merge_block)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "call_func" => {
                    // Inline probe fast-path: for 0–3 positional args with no tracing,
                    // emit Cranelift IR that checks the callable's type/arity/closure
                    // inline and calls through fn_ptr via call_indirect.  This avoids
                    // ALL function-call overhead for the common case (non-closure,
                    // exact arity, TYPE_ID_FUNCTION).  On the fast path, the generated
                    // code does: tag check -> load type_id -> load closure_bits ->
                    // load arity -> load fn_ptr -> recursion guard -> call_indirect.
                    // All loads hit the same cache line, so this is very cheap.
                    //
                    // Slow path: falls back to molt_call_func_fast{N} for closures,
                    // bound methods, arity mismatches; or molt_call_func_dispatch
                    // for >3 args or tracing.
                    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let func_bits =
                        var_get(&mut builder, &vars, &args_names[0]).expect("Func not found");
                    let mut args = Vec::new();
                    for name in &args_names[1..] {
                        args.push(*var_get(&mut builder, &vars, name).expect("Arg not found"));
                    }
                    let code_id = op.value.unwrap_or(0);
                    let nargs = args.len();

                    let use_inline_probe = nargs <= 3 && code_id == 0;

                    let res = if use_inline_probe {
                        // --- Inline probe: check tag, type_id, closure, arity ---
                        let tag_mask = builder.use_var(nbc.qnan_tag_mask);
                        let expected_ptr_tag = builder.use_var(nbc.qnan_tag_ptr);
                        let ptr_mask_val = builder.use_var(nbc.pointer_mask);

                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let slow_block = builder.create_block();

                        // Step 1: Check TAG_PTR
                        let tag = builder.ins().band(*func_bits, tag_mask);
                        let is_ptr = builder.ins().icmp(IntCC::Equal, tag, expected_ptr_tag);
                        let probe_block = builder.create_block();
                        brif_block(&mut builder, is_ptr, probe_block, &[], slow_block, &[]);

                        // Step 2: Extract pointer, check TYPE_ID_FUNCTION (221)
                        builder.switch_to_block(probe_block);
                        builder.seal_block(probe_block);
                        let raw_ptr = builder.ins().band(*func_bits, ptr_mask_val);
                        let shift16 = builder.ins().iconst(types::I64, 16);
                        let shifted = builder.ins().ishl(raw_ptr, shift16);
                        let ptr_val = builder.ins().sshr(shifted, shift16);
                        let type_id = builder.ins().load(
                            types::I32, MemFlags::trusted(), ptr_val, -16i32,
                        );
                        let expected_type = builder.ins().iconst(types::I32, 221);
                        let type_ok = builder.ins().icmp(IntCC::Equal, type_id, expected_type);
                        let closure_check_block = builder.create_block();
                        brif_block(&mut builder, type_ok, closure_check_block, &[], slow_block, &[]);

                        // Step 3: Check closure_bits == 0 (at ptr+24)
                        builder.switch_to_block(closure_check_block);
                        builder.seal_block(closure_check_block);
                        let closure_bits_v = builder.ins().load(
                            types::I64, MemFlags::trusted(), ptr_val, 24i32,
                        );
                        let zero = builder.ins().iconst(types::I64, 0);
                        let no_closure = builder.ins().icmp(IntCC::Equal, closure_bits_v, zero);
                        let arity_check_block = builder.create_block();
                        brif_block(&mut builder, no_closure, arity_check_block, &[], slow_block, &[]);

                        // Step 4: Check arity (at ptr+8)
                        builder.switch_to_block(arity_check_block);
                        builder.seal_block(arity_check_block);
                        let arity = builder.ins().load(
                            types::I64, MemFlags::trusted(), ptr_val, 8i32,
                        );
                        let expected_arity = builder.ins().iconst(types::I64, nargs as i64);
                        let arity_ok = builder.ins().icmp(IntCC::Equal, arity, expected_arity);
                        let direct_call_block = builder.create_block();
                        brif_block(&mut builder, arity_ok, direct_call_block, &[], slow_block, &[]);

                        // Step 5: Load fn_ptr (at ptr+0), recursion guard, call_indirect
                        builder.switch_to_block(direct_call_block);
                        builder.seal_block(direct_call_block);
                        let fn_ptr_v = builder.ins().load(
                            types::I64, MemFlags::trusted(), ptr_val, 0i32,
                        );
                        let guard_enter = import_func_ref(
                            &mut self.module, &mut self.import_ids, &mut builder,
                            &mut import_refs, "molt_recursion_enter_fast",
                            &[], &[types::I64],
                        );
                        let enter_call = builder.ins().call(guard_enter, &[]);
                        let guard_ok = builder.inst_results(enter_call)[0];
                        let guard_zero = builder.ins().iconst(types::I64, 0);
                        let is_guard_ok = builder.ins().icmp(IntCC::NotEqual, guard_ok, guard_zero);
                        let call_block = builder.create_block();
                        let guard_fail_block = builder.create_block();
                        brif_block(&mut builder, is_guard_ok, call_block, &[], guard_fail_block, &[]);

                        // Guard fail: raise RecursionError (cold)
                        builder.switch_to_block(guard_fail_block);
                        builder.seal_block(guard_fail_block);
                        let raise_ref = import_func_ref(
                            &mut self.module, &mut self.import_ids, &mut builder,
                            &mut import_refs, "molt_raise_recursion_error",
                            &[], &[types::I64],
                        );
                        let raise_call = builder.ins().call(raise_ref, &[]);
                        let err_val = builder.inst_results(raise_call)[0];
                        jump_block(&mut builder, merge_block, &[err_val]);

                        // Direct call via call_indirect
                        builder.switch_to_block(call_block);
                        builder.seal_block(call_block);
                        let mut call_sig = self.module.make_signature();
                        for _ in 0..nargs {
                            call_sig.params.push(AbiParam::new(types::I64));
                        }
                        call_sig.returns.push(AbiParam::new(types::I64));
                        let sig_ref = builder.import_signature(call_sig);
                        let indirect_call = builder.ins().call_indirect(sig_ref, fn_ptr_v, &args);
                        let direct_res = builder.inst_results(indirect_call)[0];
                        let guard_exit = import_func_ref(
                            &mut self.module, &mut self.import_ids, &mut builder,
                            &mut import_refs, "molt_recursion_exit_fast",
                            &[], &[],
                        );
                        builder.ins().call(guard_exit, &[]);
                        jump_block(&mut builder, merge_block, &[direct_res]);

                        // Slow path: call molt_call_func_fast{N}
                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let fast_name: &'static str = match nargs {
                            0 => "molt_call_func_fast0",
                            1 => "molt_call_func_fast1",
                            2 => "molt_call_func_fast2",
                            3 => "molt_call_func_fast3",
                            _ => unreachable!(),
                        };
                        let mut param_types = Vec::with_capacity(nargs + 1);
                        for _ in 0..=nargs {
                            param_types.push(types::I64);
                        }
                        let fast_ref = import_func_ref(
                            &mut self.module, &mut self.import_ids, &mut builder,
                            &mut import_refs, fast_name, &param_types, &[types::I64],
                        );
                        let mut slow_call_args = Vec::with_capacity(nargs + 1);
                        slow_call_args.push(*func_bits);
                        slow_call_args.extend_from_slice(&args);
                        let slow_call = builder.ins().call(fast_ref, &slow_call_args);
                        let slow_res = builder.inst_results(slow_call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        // Fallback: spill to stack + call molt_call_func_dispatch.
                        let slot_size = std::cmp::max(nargs, 1) * 8;
                        let args_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            slot_size as u32,
                            3, // align_shift: 2^3 = 8-byte alignment
                        ));
                        for (i, arg) in args.iter().enumerate() {
                            builder
                                .ins()
                                .stack_store(*arg, args_slot, (i * 8) as i32);
                        }
                        let args_ptr = builder.ins().stack_addr(types::I64, args_slot, 0);
                        let nargs_val = builder.ins().iconst(types::I64, nargs as i64);
                        let code_id_val = builder.ins().iconst(types::I64, code_id);
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_call_func_dispatch", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(
                            local_callee,
                            &[*func_bits, args_ptr, nargs_val, code_id_val],
                        );
                        builder.inst_results(call)[0]
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "invoke_ffi" => {
                    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let func_bits =
                        var_get(&mut builder, &vars, &args_names[0]).expect("Func not found");
                    let mut args = Vec::new();
                    for name in &args_names[1..] {
                        args.push(*var_get(&mut builder, &vars, name).expect("Arg not found"));
                    }
                    let callargs_new_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_callargs_new",
                        &[types::I64, types::I64],
                        &[types::I64],
                    );
                    let pos_capacity = builder.ins().iconst(types::I64, args.len() as i64);
                    let kw_capacity = builder.ins().iconst(types::I64, 0);
                    let callargs_call = builder
                        .ins()
                        .call(callargs_new_local, &[pos_capacity, kw_capacity]);
                    let callargs_ptr = builder.inst_results(callargs_call)[0];

                    let callargs_push_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_callargs_push_pos",
                        &[types::I64, types::I64],
                        &[types::I64],
                    );
                    for arg in &args {
                        builder
                            .ins()
                            .call(callargs_push_local, &[callargs_ptr, *arg]);
                    }

                    let bridge_lane = op.s_value.as_deref() == Some("bridge");
                    let call_site_label = if bridge_lane {
                        "invoke_ffi_bridge"
                    } else {
                        "invoke_ffi_deopt"
                    };
                    let site_bits = builder.ins().iconst(
                        types::I64,
                        box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            call_site_label,
                        )),
                    );
                    let require_bridge_cap = builder
                        .ins()
                        .iconst(types::I64, box_bool(if bridge_lane { 1 } else { 0 }));

                    let invoke_fn = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_invoke_ffi_ic", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let invoke_local = self.module.declare_func_in_func(invoke_fn, builder.func);
                    let invoke_call = builder.ins().call(
                        invoke_local,
                        &[site_bits, *func_bits, callargs_ptr, require_bridge_cap],
                    );
                    let res = builder.inst_results(invoke_call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "call_bind" | "call_indirect" => {
                    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let func_bits =
                        var_get(&mut builder, &vars, &args_names[0]).expect("Func not found");
                    let builder_ptr =
                        var_get(&mut builder, &vars, &args_names[1]).expect("Callargs not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee_name = if op.kind == "call_indirect" {
                        "molt_call_indirect_ic"
                    } else {
                        "molt_call_bind_ic"
                    };
                    let local_callee = if op.kind == "call_bind" {
                        import_func_ref(
                            &mut self.module,
                            &mut self.import_ids,
                            &mut builder,
                            &mut import_refs,
                            "molt_call_bind_ic",
                            &[types::I64, types::I64, types::I64],
                            &[types::I64],
                        )
                    } else {
                        let callee = self
                            .module
                            .declare_function(callee_name, Linkage::Import, &sig)
                            .unwrap();
                        self.module.declare_func_in_func(callee, builder.func)
                    };
                    let call_site_label = if op.kind == "call_indirect" {
                        "call_indirect"
                    } else {
                        "call_bind"
                    };
                    let site_bits = builder.ins().iconst(
                        types::I64,
                        box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            call_site_label,
                        )),
                    );
                    let call = builder
                        .ins()
                        .call(local_callee, &[site_bits, *func_bits, *builder_ptr]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }

                    // `molt_call_bind*` consumes the CallArgs builder pointer and decrefs it
                    // internally (see `PtrDropGuard` in runtime). The backend's lifetime tracking
                    // must therefore *not* emit an additional decref for the builder variable,
                    // or we'll double-free the CallArgs object and corrupt unrelated state.
                    //
                    // call_bind consumes the callargs builder. Remove it from
                    // tracking to prevent double-free. The last_use assertion is
                    // omitted: the IR may reference the variable in unreachable
                    // branches (different if/else arms), inflating last_use.
                    let callargs_name = &args_names[1];
                    if let Some(block) = builder.current_block() {
                        if block == entry_block && loop_depth == 0 {
                            tracked_obj_vars.retain(|n| n != callargs_name);
                            tracked_vars.retain(|n| n != callargs_name);
                            tracked_obj_vars_set.remove(callargs_name);
                            tracked_vars_set.remove(callargs_name);
                            entry_vars.remove(callargs_name);
                        } else {
                            if let Some(names) = block_tracked_obj.get_mut(&block) {
                                names.retain(|n| n != callargs_name);
                            }
                            if let Some(names) = block_tracked_ptr.get_mut(&block) {
                                names.retain(|n| n != callargs_name);
                            }
                        }
                    }
                }
                "call_method" => {
                    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let method_bits =
                        var_get(&mut builder, &vars, &args_names[0]).expect("Method not found");
                    let mut extra_args = Vec::new();
                    for name in &args_names[1..] {
                        extra_args
                            .push(*var_get(&mut builder, &vars, name).expect("Arg not found"));
                    }
                    let callargs_new_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_callargs_new",
                        &[types::I64, types::I64],
                        &[types::I64],
                    );
                    let pos_capacity = builder.ins().iconst(types::I64, extra_args.len() as i64);
                    let kw_capacity = builder.ins().iconst(types::I64, 0);
                    let callargs_call = builder
                        .ins()
                        .call(callargs_new_local, &[pos_capacity, kw_capacity]);
                    let callargs_ptr = builder.inst_results(callargs_call)[0];
                    let callargs_push_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_callargs_push_pos",
                        &[types::I64, types::I64],
                        &[types::I64],
                    );
                    for arg in &extra_args {
                        builder
                            .ins()
                            .call(callargs_push_local, &[callargs_ptr, *arg]);
                    }
                    let call_bind_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_call_bind_ic",
                        &[types::I64, types::I64, types::I64],
                        &[types::I64],
                    );
                    let site_bits = builder.ins().iconst(
                        types::I64,
                        box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            "call_method",
                        )),
                    );
                    let call = builder
                        .ins()
                        .call(call_bind_local, &[site_bits, *method_bits, callargs_ptr]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "module_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let name_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module name not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_module_new", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*name_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "class_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let name_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class name not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_class_new", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*name_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                // Outlined class definition via molt_guarded_class_def
                "class_def" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let meta = op.s_value.as_ref().expect("class_def needs s_value");
                    let parts: Vec<&str> = meta.split(',').collect();
                    let nbases: usize = parts[0].parse().unwrap();
                    let nattrs: usize = parts[1].parse().unwrap();
                    let layout_size: i64 = parts[2].parse().unwrap();
                    let layout_version: i64 = parts[3].parse().unwrap();
                    let flags: i64 = parts[4].parse().unwrap();
                    let name_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class name not found");
                    let bases_slot_size = std::cmp::max(nbases, 1) * 8;
                    let bases_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        bases_slot_size as u32,
                        3,
                    ));
                    for i in 0..nbases {
                        let base = var_get(&mut builder, &vars, &args[1 + i])
                            .expect("Base class not found");
                        builder.ins().stack_store(*base, bases_slot, (i * 8) as i32);
                    }
                    let bases_ptr = builder.ins().stack_addr(types::I64, bases_slot, 0);
                    let attrs_slot_size = std::cmp::max(nattrs * 2, 1) * 8;
                    let attrs_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        attrs_slot_size as u32,
                        3,
                    ));
                    let attrs_base = 1 + nbases;
                    for i in 0..nattrs {
                        let key = var_get(&mut builder, &vars, &args[attrs_base + i * 2])
                            .expect("Attr key not found");
                        let val = var_get(&mut builder, &vars, &args[attrs_base + i * 2 + 1])
                            .expect("Attr value not found");
                        builder
                            .ins()
                            .stack_store(*key, attrs_slot, (i * 2 * 8) as i32);
                        builder
                            .ins()
                            .stack_store(*val, attrs_slot, ((i * 2 + 1) * 8) as i32);
                    }
                    let attrs_ptr = builder.ins().stack_addr(types::I64, attrs_slot, 0);
                    let nbases_val = builder.ins().iconst(types::I64, nbases as i64);
                    let nattrs_val = builder.ins().iconst(types::I64, nattrs as i64);
                    let layout_size_val = builder.ins().iconst(types::I64, layout_size);
                    let layout_version_val = builder.ins().iconst(types::I64, layout_version);
                    let flags_val = builder.ins().iconst(types::I64, flags);
                    let cd_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_guarded_class_def", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let cd_local =
                        self.module.declare_func_in_func(cd_callee, builder.func);
                    let cd_call = builder.ins().call(
                        cd_local,
                        &[
                            *name_bits, bases_ptr, nbases_val,
                            attrs_ptr, nattrs_val,
                            layout_size_val, layout_version_val, flags_val,
                        ],
                    );
                    let res = builder.inst_results(cd_call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "builtin_type" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let tag_bits = var_get(&mut builder, &vars, &args[0]).expect("Tag not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_builtin_type", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*tag_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "type_of" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_type_of", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "is_native_awaitable" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_is_native_awaitable", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "class_layout_version" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_class_layout_version", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*class_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "class_set_layout_version" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let version_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Version not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_class_set_layout_version", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*class_bits, *version_bits]);
                    if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, out_name.clone(), res);
                    }
                }
                "isinstance" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_isinstance", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj_bits, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "issubclass" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let sub_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Subclass not found");
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_issubclass", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*sub_bits, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "object_new" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_object_new", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "class_set_base" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let base_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Base class not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_class_set_base", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*class_bits, *base_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "class_apply_set_name" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_class_apply_set_name", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*class_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "super_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let type_bits = var_get(&mut builder, &vars, &args[0]).expect("Type not found");
                    let obj_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Object not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_super_new", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*type_bits, *obj_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "classmethod_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let func_bits = var_get(&mut builder, &vars, &args[0]).expect("Func not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_classmethod_new", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*func_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "staticmethod_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let func_bits = var_get(&mut builder, &vars, &args[0]).expect("Func not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_staticmethod_new", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*func_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "property_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let getter = var_get(&mut builder, &vars, &args[0]).expect("Getter not found");
                    let setter = var_get(&mut builder, &vars, &args[1]).expect("Setter not found");
                    let deleter =
                        var_get(&mut builder, &vars, &args[2]).expect("Deleter not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_property_new", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*getter, *setter, *deleter]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "object_set_class" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj_bits, &nbc);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_object_set_class", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[obj_ptr, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "module_cache_get" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let name_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module name not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_module_cache_get", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*name_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "module_import" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let name_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module name not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_module_import", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*name_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "module_cache_set" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let name_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module name not found");
                    let module_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Module not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_module_cache_set", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder
                        .ins()
                        .call(local_callee, &[*name_bits, *module_bits]);
                }
                "module_cache_del" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let name_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module name not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_module_cache_del", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*name_bits]);
                }
                "module_get_attr" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let module_bits = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!(
                            "Module not found in {} op {} ({:?})",
                            func_ir.name, op_idx, op.args
                        )
                    });
                    let attr_bits = var_get(&mut builder, &vars, &args[1]).unwrap_or_else(|| {
                        panic!(
                            "Attr not found in {} op {} ({:?})",
                            func_ir.name, op_idx, op.args
                        )
                    });
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_module_get_attr", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*module_bits, *attr_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "module_get_global" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let module_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module not found");
                    let attr_bits = var_get(&mut builder, &vars, &args[1]).expect("Attr not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_module_get_global", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*module_bits, *attr_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "module_del_global" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let module_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module not found");
                    let attr_bits = var_get(&mut builder, &vars, &args[1]).expect("Attr not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_module_del_global", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*module_bits, *attr_bits]);
                    if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, out_name.clone(), res);
                    }
                }
                "module_get_name" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let module_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module not found");
                    let attr_bits = var_get(&mut builder, &vars, &args[1]).expect("Attr not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_module_get_name", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*module_bits, *attr_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "module_set_attr" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let module_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module not found");
                    let attr_bits = var_get(&mut builder, &vars, &args[1]).expect("Attr not found");
                    let val_bits = var_get(&mut builder, &vars, &args[2]).unwrap_or_else(|| {
                        panic!(
                            "Value not found for module_set_attr in {} op {}",
                            func_ir.name, op_idx
                        )
                    });
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_module_set_attr", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder
                        .ins()
                        .call(local_callee, &[*module_bits, *attr_bits, *val_bits]);
                }
                "module_import_star" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let src_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module not found");
                    let dst_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Module not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_module_import_star", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*src_bits, *dst_bits]);
                }
                "context_null" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let payload =
                        var_get(&mut builder, &vars, &args[0]).expect("Payload not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_context_null", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*payload]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "context_enter" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let ctx = var_get(&mut builder, &vars, &args[0]).expect("Context not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_context_enter", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*ctx]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "context_exit" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let ctx = var_get(&mut builder, &vars, &args[0]).expect("Context not found");
                    let exc = var_get(&mut builder, &vars, &args[1]).expect("Exception not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_context_exit", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*ctx, *exc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "context_closing" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let payload =
                        var_get(&mut builder, &vars, &args[0]).expect("Payload not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_context_closing", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*payload]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "context_unwind" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_context_unwind", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "context_depth" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_context_depth", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "context_unwind_to" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let depth = var_get(&mut builder, &vars, &args[0]).expect("Depth not found");
                    let exc = var_get(&mut builder, &vars, &args[1]).expect("Exception not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_context_unwind_to", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*depth, *exc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_push" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_push", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_pop" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_pop", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_stack_clear" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_stack_clear", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    if let Some(out_name) = op.out {
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, out_name, res);
                    }
                }
                "exception_stack_depth" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_stack_depth", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_stack_enter" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_stack_enter", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_stack_exit" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let prev = var_get(&mut builder, &vars, &args[0])
                        .expect("exception baseline not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_stack_exit", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*prev]);
                    if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, out_name.clone(), res);
                    }
                }
                "exception_stack_set_depth" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let depth =
                        var_get(&mut builder, &vars, &args[0]).expect("exception depth not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_stack_set_depth", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*depth]);
                    if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, out_name.clone(), res);
                    }
                }
                "exception_last" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_last", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "getargv" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_getargv", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "getframe" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let depth = var_get(&mut builder, &vars, &args[0]).expect("depth not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_getframe", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*depth]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "sys_executable" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_sys_executable", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_new" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let kind = var_get(&mut builder, &vars, &args[0]).expect("Kind not found");
                    let args_bits = var_get(&mut builder, &vars, &args[1]).expect("Args not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_new", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*kind, *args_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_new_from_class" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let args_bits = var_get(&mut builder, &vars, &args[1]).expect("Args not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_new_from_class", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*class_bits, *args_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exceptiongroup_match" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let matcher =
                        var_get(&mut builder, &vars, &args[1]).expect("Matcher not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exceptiongroup_match", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc, *matcher]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exceptiongroup_combine" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let items =
                        var_get(&mut builder, &vars, &args[0]).expect("Exception list not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exceptiongroup_combine", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*items]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_clear" => {
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_clear", &[], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_kind" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_kind", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_class" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let kind =
                        var_get(&mut builder, &vars, &args[0]).expect("Exception kind not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_class", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*kind]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_message" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_message", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_set_cause" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let cause = var_get(&mut builder, &vars, &args[1]).expect("Cause not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_set_cause", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc, *cause]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_set_last" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_set_last", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_set_value" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let value = var_get(&mut builder, &vars, &args[1]).expect("Value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_set_value", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc, *value]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "exception_context_set" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_exception_context_set", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "raise" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_raise", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out) = op.out.as_ref()
                        && out != "none"
                    {
                        def_var_named(&mut builder, &vars, out.clone(), res);
                    }
                }
                "check_exception" => {
                    let target_id = op.value.unwrap_or(0);
                    let Some(&target_block) = state_blocks.get(&target_id) else {
                        // Orphaned check_exception (handler stripped by IR pass) — skip.
                        continue;
                    };
                    let mut carry_obj: Vec<String> = Vec::new();
                    let mut carry_ptr: Vec<String> = Vec::new();
                    // `check_exception` terminates the current block (brif) to either jump to the
                    // exception handler label or continue on the fallthrough path. That means any
                    // temporaries tracked on the current block would otherwise have no natural
                    // "line"/control-flow cleanup point until much later. Drain dead values here so
                    // short-lived temporaries (for example list indexing results) are decref'd
                    // deterministically and do not leak across exception checks.
                    if let Some(block) = builder.current_block() {
                        if let Some(names) = block_tracked_obj.remove(&block) {
                            carry_obj.extend(names);
                        }
                        if let Some(names) = block_tracked_ptr.remove(&block) {
                            carry_ptr.extend(names);
                        }
                        if block == entry_block && loop_depth == 0 {
                            carry_obj.append(&mut tracked_obj_vars);
                            carry_ptr.append(&mut tracked_vars);
                            tracked_obj_vars_set.clear();
                            tracked_vars_set.clear();
                        }
                        if std::env::var("MOLT_DEBUG_CHECK_EXCEPTION").as_deref() == Ok("1")
                            && std::env::var("MOLT_DEBUG_FUNC_FILTER")
                                .ok()
                                .map_or(true, |f| func_ir.name.contains(&f))
                        {
                            eprintln!("check_exception {} op={}", func_ir.name, op_idx,);
                        }
                    }
                    // Collect all names scrubbed from both obj and ptr so we can
                    // do a single pass over block_tracked maps instead of O(n*m).
                    let mut scrubbed_names: std::collections::HashSet<String> = std::collections::HashSet::new();
                    if !carry_obj.is_empty() {
                        let cleanup =
                            drain_cleanup_tracked(&mut carry_obj, &last_use, op_idx, None);
                        for name in cleanup {
                            // Use entry_vars (definition-time Value) for dec_ref,
                            // not var_get (current SSA Value). If the variable was
                            // redefined, var_get returns the WRONG object.
                            let val = entry_vars.get(&name).copied()
                                .or_else(|| var_get(&mut builder, &vars, &name).map(|v| *v));
                            let Some(val) = val else { continue; };
                            builder.ins().call(local_dec_ref_obj, &[val]);
                            entry_vars.remove(&name);
                            scrubbed_names.insert(name);
                        }
                    }
                    if !carry_ptr.is_empty() {
                        let cleanup =
                            drain_cleanup_tracked(&mut carry_ptr, &last_use, op_idx, None);
                        for name in cleanup {
                            let val = entry_vars.get(&name).copied()
                                .or_else(|| var_get(&mut builder, &vars, &name).map(|v| *v));
                            let Some(val) = val else { continue; };
                            builder.ins().call(local_dec_ref_obj, &[val]);
                            entry_vars.remove(&name);
                            scrubbed_names.insert(name);
                        }
                    }
                    // Single pass over all exception handler blocks to remove
                    // scrubbed names, instead of one retain per name per block.
                    if !scrubbed_names.is_empty() {
                        for tracked_list in block_tracked_obj.values_mut() {
                            tracked_list.retain(|n| !scrubbed_names.contains(n));
                        }
                        for tracked_list in block_tracked_ptr.values_mut() {
                            tracked_list.retain(|n| !scrubbed_names.contains(n));
                        }
                    }
                    // Inline exception check: load the pending flag byte directly
                    // instead of calling molt_exception_pending_fast() for each
                    // check_exception site.  The flag pointer is fetched once per
                    // block and the byte load is ~1 cycle vs ~15-40 cycles for the
                    // function call.
                    //
                    // The flag pointer lives in a Cranelift Variable (SSA
                    // propagates it across all blocks automatically, including
                    // stateful/poll functions).  The per-block cache is a
                    // fallback for any edge case where the Variable is unavailable.
                    let fallthrough = builder.create_block();
                    reachable_blocks.insert(target_block);
                    reachable_blocks.insert(fallthrough);
                    // Resolve the flag pointer for this check_exception site.
                    let flag_ptr_val: Option<Value> = if let Some(var) = exc_flag_ptr_var {
                        // Non-stateful path: use the Cranelift Variable.
                        Some(builder.use_var(var))
                    } else if let Some(fn_ref) = exc_flag_ptr_fn {
                        // Stateful path: fetch pointer once per block, cache it.
                        let current_block = builder.current_block().unwrap();
                        let ptr = if let Some(&cached) = exc_flag_ptr_block_cache.get(&current_block) {
                            cached
                        } else {
                            let call = builder.ins().call(fn_ref, &[]);
                            let ptr = builder.inst_results(call)[0];
                            exc_flag_ptr_block_cache.insert(current_block, ptr);
                            ptr
                        };
                        Some(ptr)
                    } else {
                        None
                    };
                    if let Some(flag_ptr) = flag_ptr_val {
                        // Fast path: inline byte load from flag address
                        let pending_byte = builder.ins().load(
                            types::I8,
                            MemFlags::trusted(),
                            flag_ptr,
                            0,
                        );
                        let pending_i64 = builder.ins().uextend(types::I64, pending_byte);
                        let is_pending = builder.ins().icmp_imm(IntCC::NotEqual, pending_i64, 0);
                        // On positive read, validate with full function before branching
                        let validate_block = builder.create_block();
                        reachable_blocks.insert(validate_block);
                        brif_block(&mut builder, is_pending, validate_block, &[], fallthrough, &[]);
                        switch_to_block_tracking(&mut builder, validate_block, &mut is_block_filled);
                        let call = builder.ins().call(local_exc_pending_fast, &[]);
                        let confirmed = builder.inst_results(call)[0];
                        let cond2 = builder.ins().icmp_imm(IntCC::NotEqual, confirmed, 0);
                        brif_block(&mut builder, cond2, target_block, &[], fallthrough, &[]);
                        builder.seal_block(validate_block);
                    } else {
                        // Fallback: direct function call (no flag pointer available)
                        let call = builder.ins().call(local_exc_pending_fast, &[]);
                        let pending = builder.inst_results(call)[0];
                        let cond = builder.ins().icmp_imm(IntCC::NotEqual, pending, 0);
                        brif_block(&mut builder, cond, target_block, &[], fallthrough, &[]);
                    }
                    switch_to_block_tracking(&mut builder, fallthrough, &mut is_block_filled);
                    // Defer sealing to seal_all_blocks() — early sealing breaks
                    // SSA variable propagation for loop counters through fallthrough blocks.
                    // check_exception's fallthrough is always a fresh empty block —
                    // force-clear is_block_filled so subsequent ops (add, loop_index_next)
                    // are never incorrectly skipped by the whitelist guard.
                    is_block_filled = false;
                    // Propagate remaining tracked objects to BOTH the fallthrough
                    // and the exception handler. Without this, the exception handler
                    // may access objects that were only passed to the fallthrough,
                    // causing use-after-free when the exception handler dec-refs them.
                    if !carry_obj.is_empty() {
                        block_tracked_obj
                            .entry(fallthrough)
                            .or_default()
                            .extend(carry_obj.clone());
                        extend_unique_tracked(
                            block_tracked_obj.entry(target_block).or_default(),
                            carry_obj,
                        );
                    }
                    if !carry_ptr.is_empty() {
                        block_tracked_ptr
                            .entry(fallthrough)
                            .or_default()
                            .extend(carry_ptr.clone());
                        extend_unique_tracked(
                            block_tracked_ptr.entry(target_block).or_default(),
                            carry_ptr,
                        );
                    }
                }
                "file_open" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let path = var_get(&mut builder, &vars, &args[0]).expect("Path not found");
                    let mode = var_get(&mut builder, &vars, &args[1]).expect("Mode not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_file_open", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*path, *mode]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "file_read" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let handle = var_get(&mut builder, &vars, &args[0]).expect("Handle not found");
                    let size = var_get(&mut builder, &vars, &args[1]).expect("Size not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_file_read", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*handle, *size]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "file_write" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let handle = var_get(&mut builder, &vars, &args[0]).expect("Handle not found");
                    let data = var_get(&mut builder, &vars, &args[1]).expect("Data not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_file_write", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*handle, *data]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "file_close" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let handle = var_get(&mut builder, &vars, &args[0]).expect("Handle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_file_close", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*handle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "file_flush" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let handle = var_get(&mut builder, &vars, &args[0]).expect("Handle not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_file_flush", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*handle]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "bridge_unavailable" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let msg = var_get(&mut builder, &vars, &args[0]).expect("Message not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_bridge_unavailable", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*msg]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "if" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let cond = var_get(&mut builder, &vars, &args[0]).expect("Cond not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_is_truthy", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*cond]);
                    let truthy = builder.inst_results(call)[0];
                    let cond_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0);
                    // `if` terminates the current block (brif) into then/else blocks. Any live
                    // tracked values must be carried into both successors; otherwise they leak
                    // when the predecessor block is never revisited.
                    let origin_block = builder
                        .current_block()
                        .expect("if requires an active block");
                    let mut carry_obj = block_tracked_obj.remove(&origin_block).unwrap_or_default();
                    let cleanup_obj =
                        drain_cleanup_tracked(&mut carry_obj, &last_use, op_idx, None);
                    for name in cleanup_obj {
                        let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                            panic!(
                                "Tracked obj var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                    let mut carry_ptr = block_tracked_ptr.remove(&origin_block).unwrap_or_default();
                    let cleanup_ptr =
                        drain_cleanup_tracked(&mut carry_ptr, &last_use, op_idx, None);
                    for name in cleanup_ptr {
                        let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                            panic!(
                                "Tracked ptr var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                    let has_explicit_else = if_to_else.contains_key(&op_idx);
                    let end_if_idx = match if_to_end_if.get(&op_idx) {
                        Some(&idx) => idx,
                        None => {
                            eprintln!(
                                "WARNING: `if` at op {} in function `{}` has no matching end_if — skipping",
                                op_idx, func_ir.name
                            );
                            continue;
                        }
                    };
                    let has_phi_join = func_ir
                        .ops
                        .get(end_if_idx + 1)
                        .is_some_and(|next| next.kind == "phi");
                    let then_block = builder.create_block();
                    let else_block = if has_explicit_else || has_phi_join {
                        Some(builder.create_block())
                    } else {
                        None
                    };
                    let merge_block = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(then_block, current_block);
                        if let Some(else_block) = else_block {
                            builder.insert_block_after(else_block, then_block);
                        }
                    }
                    reachable_blocks.insert(then_block);
                    if let Some(else_block) = else_block {
                        reachable_blocks.insert(else_block);
                    }
                    if !carry_obj.is_empty() {
                        extend_unique_tracked(
                            block_tracked_obj.entry(then_block).or_default(),
                            carry_obj.clone(),
                        );
                        if let Some(else_block) = else_block {
                            extend_unique_tracked(
                                block_tracked_obj.entry(else_block).or_default(),
                                carry_obj.clone(),
                            );
                        } else {
                            extend_unique_tracked(
                                block_tracked_obj.entry(merge_block).or_default(),
                                carry_obj.clone(),
                            );
                        }
                    }
                    if !carry_ptr.is_empty() {
                        extend_unique_tracked(
                            block_tracked_ptr.entry(then_block).or_default(),
                            carry_ptr.clone(),
                        );
                        if let Some(else_block) = else_block {
                            extend_unique_tracked(
                                block_tracked_ptr.entry(else_block).or_default(),
                                carry_ptr.clone(),
                            );
                        } else {
                            extend_unique_tracked(
                                block_tracked_ptr.entry(merge_block).or_default(),
                                carry_ptr.clone(),
                            );
                        }
                    }
                    let false_block = else_block.unwrap_or(merge_block);
                    if else_block.is_none() {
                        reachable_blocks.insert(merge_block);
                    }
                    builder
                        .ins()
                        .brif(cond_bool, then_block, &[], false_block, &[]);

                    // Seal blocks now that their predecessor sets are complete.
                    // Structured `if` creates exactly one predecessor for each of then/else.
                    //
                    // Note: we deliberately do not seal `origin_block` here because it may have
                    // been sealed earlier (for example the function entry block is sealed up-front).
                    if sealed_blocks.insert(then_block) {
                        builder.seal_block(then_block);
                    }
                    if let Some(else_block) = else_block
                        && sealed_blocks.insert(else_block)
                    {
                        builder.seal_block(else_block);
                    }

                    switch_to_block_tracking(&mut builder, then_block, &mut is_block_filled);
                    if_stack.push(IfFrame {
                        else_block,
                        merge_block,
                        has_else: false,
                        then_terminal: false,
                        else_terminal: false,
                        phi_ops: Vec::new(),
                        phi_params: Vec::new(),
                    });
                }
                "else" => {
                    let frame = if_stack.last_mut().expect("No if on stack");
                    frame.then_terminal = is_block_filled;
                    if frame.phi_ops.is_empty() {
                        let end_if_idx = *else_to_end_if
                            .get(&op_idx)
                            .expect("else without matching end_if");
                        let mut phi_ops: Vec<(String, String, String)> = Vec::new();
                        let mut scan_idx = end_if_idx + 1;
                        while scan_idx < ops.len() {
                            let next = &ops[scan_idx];
                            if next.kind != "phi" {
                                break;
                            }
                            let args = next.args.as_ref().expect("phi args missing");
                            if args.len() != 2 {
                                panic!("phi expects exactly two args");
                            }
                            let out = next.out.clone().expect("phi output missing");
                            phi_ops.push((out, args[0].clone(), args[1].clone()));
                            skip_ops.insert(scan_idx);
                            scan_idx += 1;
                        }
                        frame.phi_ops = phi_ops;
                    }

                    if !is_block_filled {
                        // If this structured `if` is followed by `phi` ops, route values through
                        // merge-block parameters (real SSA join) instead of attempting to "define"
                        // the output in each predecessor block.
                        let mut phi_args: Vec<Value> = Vec::new();
                        if !frame.phi_ops.is_empty() {
                            if frame.phi_params.is_empty() {
                                for (_out, then_name, _else_name) in &frame.phi_ops {
                                    let then_val = var_get(&mut builder, &vars, then_name)
                                        .unwrap_or_else(|| {
                                            panic!("phi arg not found: {then_name}")
                                        });
                                    let ty = builder.func.dfg.value_type(*then_val);
                                    let param = builder.append_block_param(frame.merge_block, ty);
                                    frame.phi_params.push(param);
                                    phi_args.push(*then_val);
                                }
                            } else {
                                for (_out, then_name, _else_name) in &frame.phi_ops {
                                    let then_val = var_get(&mut builder, &vars, then_name)
                                        .unwrap_or_else(|| {
                                            panic!("phi arg not found: {then_name}")
                                        });
                                    phi_args.push(*then_val);
                                }
                            }
                        }
                        if let Some(block) = builder.current_block() {
                            let mut carry_obj =
                                block_tracked_obj.remove(&block).unwrap_or_default();
                            let cleanup =
                                drain_cleanup_tracked(&mut carry_obj, &last_use, op_idx, None);
                            for name in cleanup {
                                let val =
                                    var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                        panic!(
                                            "Tracked obj var not found in {} op {}: {}",
                                            func_ir.name, op_idx, name
                                        )
                                    });
                                builder.ins().call(local_dec_ref_obj, &[*val]);
                            }
                            if !carry_obj.is_empty() {
                                extend_unique_tracked(
                                    block_tracked_obj.entry(frame.merge_block).or_default(),
                                    carry_obj,
                                );
                            }

                            let mut carry_ptr =
                                block_tracked_ptr.remove(&block).unwrap_or_default();
                            let cleanup =
                                drain_cleanup_tracked(&mut carry_ptr, &last_use, op_idx, None);
                            for name in cleanup {
                                let val =
                                    var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                        panic!(
                                            "Tracked ptr var not found in {} op {}: {}",
                                            func_ir.name, op_idx, name
                                        )
                                    });
                                builder.ins().call(local_dec_ref_obj, &[*val]);
                            }
                            if !carry_ptr.is_empty() {
                                extend_unique_tracked(
                                    block_tracked_ptr.entry(frame.merge_block).or_default(),
                                    carry_ptr,
                                );
                            }
                            ensure_block_in_layout(&mut builder, frame.merge_block);
                            reachable_blocks.insert(frame.merge_block);
                            jump_block(&mut builder, frame.merge_block, &phi_args);
                        }
                    }

                    switch_to_block_tracking(
                        &mut builder,
                        frame.else_block.expect("else without placeholder block"),
                        &mut is_block_filled,
                    );
                    frame.has_else = true;
                }
                "end_if" => {
                    let mut frame = if_stack.pop().expect("No if on stack");
                    if frame.phi_ops.is_empty() {
                        let mut phi_ops: Vec<(String, String, String)> = Vec::new();
                        let mut scan_idx = op_idx + 1;
                        while scan_idx < ops.len() {
                            let next = &ops[scan_idx];
                            if next.kind != "phi" {
                                break;
                            }
                            let args = next.args.as_ref().expect("phi args missing");
                            if args.len() != 2 {
                                panic!("phi expects exactly two args");
                            }
                            let out = next.out.clone().expect("phi output missing");
                            phi_ops.push((out, args[0].clone(), args[1].clone()));
                            skip_ops.insert(scan_idx);
                            scan_idx += 1;
                        }
                        frame.phi_ops = phi_ops;
                    }

                    if frame.has_else {
                        frame.else_terminal = is_block_filled;
                        if !is_block_filled {
                            let mut phi_args: Vec<Value> = Vec::new();
                            if !frame.phi_ops.is_empty() {
                                if frame.phi_params.is_empty() {
                                    for (_out, _then_name, else_name) in &frame.phi_ops {
                                        let else_val = var_get(&mut builder, &vars, else_name)
                                            .unwrap_or_else(|| {
                                                panic!("phi arg not found: {else_name}")
                                            });
                                        let ty = builder.func.dfg.value_type(*else_val);
                                        let param =
                                            builder.append_block_param(frame.merge_block, ty);
                                        frame.phi_params.push(param);
                                        phi_args.push(*else_val);
                                    }
                                } else {
                                    for (_out, _then_name, else_name) in &frame.phi_ops {
                                        let else_val = var_get(&mut builder, &vars, else_name)
                                            .unwrap_or_else(|| {
                                                panic!("phi arg not found: {else_name}")
                                            });
                                        phi_args.push(*else_val);
                                    }
                                }
                            }
                            if let Some(block) = builder.current_block() {
                                let mut carry_obj =
                                    block_tracked_obj.remove(&block).unwrap_or_default();
                                let cleanup =
                                    drain_cleanup_tracked(&mut carry_obj, &last_use, op_idx, None);
                                for name in cleanup {
                                    let val =
                                        var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                            panic!(
                                                "Tracked obj var not found in {} op {}: {}",
                                                func_ir.name, op_idx, name
                                            )
                                        });
                                    builder.ins().call(local_dec_ref_obj, &[*val]);
                                }
                                if !carry_obj.is_empty() {
                                    extend_unique_tracked(
                                        block_tracked_obj.entry(frame.merge_block).or_default(),
                                        carry_obj,
                                    );
                                }

                                let mut carry_ptr =
                                    block_tracked_ptr.remove(&block).unwrap_or_default();
                                let cleanup =
                                    drain_cleanup_tracked(&mut carry_ptr, &last_use, op_idx, None);
                                for name in cleanup {
                                    let val =
                                        var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                            panic!(
                                                "Tracked ptr var not found in {} op {}: {}",
                                                func_ir.name, op_idx, name
                                            )
                                        });
                                    builder.ins().call(local_dec_ref_obj, &[*val]);
                                }
                                if !carry_ptr.is_empty() {
                                    extend_unique_tracked(
                                        block_tracked_ptr.entry(frame.merge_block).or_default(),
                                        carry_ptr,
                                    );
                                }
                                ensure_block_in_layout(&mut builder, frame.merge_block);
                                reachable_blocks.insert(frame.merge_block);
                                jump_block(&mut builder, frame.merge_block, &phi_args);
                            }
                        }
                    } else {
                        frame.then_terminal = is_block_filled;
                        frame.else_terminal = false;
                        if !is_block_filled {
                            let mut phi_args: Vec<Value> = Vec::new();
                            if !frame.phi_ops.is_empty() {
                                if frame.phi_params.is_empty() {
                                    for (_out, then_name, _else_name) in &frame.phi_ops {
                                        let then_val = var_get(&mut builder, &vars, then_name)
                                            .unwrap_or_else(|| {
                                                panic!("phi arg not found: {then_name}")
                                            });
                                        let ty = builder.func.dfg.value_type(*then_val);
                                        let param =
                                            builder.append_block_param(frame.merge_block, ty);
                                        frame.phi_params.push(param);
                                        phi_args.push(*then_val);
                                    }
                                } else {
                                    for (_out, then_name, _else_name) in &frame.phi_ops {
                                        let then_val = var_get(&mut builder, &vars, then_name)
                                            .unwrap_or_else(|| {
                                                panic!("phi arg not found: {then_name}")
                                            });
                                        phi_args.push(*then_val);
                                    }
                                }
                            }
                            if let Some(block) = builder.current_block() {
                                let mut carry_obj =
                                    block_tracked_obj.remove(&block).unwrap_or_default();
                                let cleanup =
                                    drain_cleanup_tracked(&mut carry_obj, &last_use, op_idx, None);
                                for name in cleanup {
                                    let val =
                                        var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                            panic!(
                                                "Tracked obj var not found in {} op {}: {}",
                                                func_ir.name, op_idx, name
                                            )
                                        });
                                    builder.ins().call(local_dec_ref_obj, &[*val]);
                                }
                                if !carry_obj.is_empty() {
                                    extend_unique_tracked(
                                        block_tracked_obj.entry(frame.merge_block).or_default(),
                                        carry_obj,
                                    );
                                }

                                let mut carry_ptr =
                                    block_tracked_ptr.remove(&block).unwrap_or_default();
                                let cleanup =
                                    drain_cleanup_tracked(&mut carry_ptr, &last_use, op_idx, None);
                                for name in cleanup {
                                    let val =
                                        var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                            panic!(
                                                "Tracked ptr var not found in {} op {}: {}",
                                                func_ir.name, op_idx, name
                                            )
                                        });
                                    builder.ins().call(local_dec_ref_obj, &[*val]);
                                }
                                if !carry_ptr.is_empty() {
                                    extend_unique_tracked(
                                        block_tracked_ptr.entry(frame.merge_block).or_default(),
                                        carry_ptr,
                                    );
                                }
                                ensure_block_in_layout(&mut builder, frame.merge_block);
                                reachable_blocks.insert(frame.merge_block);
                                jump_block(&mut builder, frame.merge_block, &phi_args);
                            }
                        }

                        if let Some(else_block) = frame.else_block {
                            switch_to_block_tracking(
                                &mut builder,
                                else_block,
                                &mut is_block_filled,
                            );
                            let mut phi_args: Vec<Value> = Vec::new();
                            if !frame.phi_ops.is_empty() {
                                if frame.phi_params.is_empty() {
                                    for (_out, _then_name, else_name) in &frame.phi_ops {
                                        let else_val = var_get(&mut builder, &vars, else_name)
                                            .unwrap_or_else(|| {
                                                panic!("phi arg not found: {else_name}")
                                            });
                                        let ty = builder.func.dfg.value_type(*else_val);
                                        let param =
                                            builder.append_block_param(frame.merge_block, ty);
                                        frame.phi_params.push(param);
                                        phi_args.push(*else_val);
                                    }
                                } else {
                                    for (_out, _then_name, else_name) in &frame.phi_ops {
                                        let else_val = var_get(&mut builder, &vars, else_name)
                                            .unwrap_or_else(|| {
                                                panic!("phi arg not found: {else_name}")
                                            });
                                        phi_args.push(*else_val);
                                    }
                                }
                            }
                            if let Some(block) = builder.current_block() {
                                let mut carry_obj =
                                    block_tracked_obj.remove(&block).unwrap_or_default();
                                let cleanup =
                                    drain_cleanup_tracked(&mut carry_obj, &last_use, op_idx, None);
                                for name in cleanup {
                                    let val =
                                        var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                            panic!(
                                                "Tracked obj var not found in {} op {}: {}",
                                                func_ir.name, op_idx, name
                                            )
                                        });
                                    builder.ins().call(local_dec_ref_obj, &[*val]);
                                }
                                if !carry_obj.is_empty() {
                                    extend_unique_tracked(
                                        block_tracked_obj.entry(frame.merge_block).or_default(),
                                        carry_obj,
                                    );
                                }

                                let mut carry_ptr =
                                    block_tracked_ptr.remove(&block).unwrap_or_default();
                                let cleanup =
                                    drain_cleanup_tracked(&mut carry_ptr, &last_use, op_idx, None);
                                for name in cleanup {
                                    let val =
                                        var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                            panic!(
                                                "Tracked ptr var not found in {} op {}: {}",
                                                func_ir.name, op_idx, name
                                            )
                                        });
                                    builder.ins().call(local_dec_ref_obj, &[*val]);
                                }
                                if !carry_ptr.is_empty() {
                                    extend_unique_tracked(
                                        block_tracked_ptr.entry(frame.merge_block).or_default(),
                                        carry_ptr,
                                    );
                                }
                            }
                            ensure_block_in_layout(&mut builder, frame.merge_block);
                            reachable_blocks.insert(frame.merge_block);
                            jump_block(&mut builder, frame.merge_block, &phi_args);
                        }
                    }

                    let both_filled = frame.then_terminal && frame.else_terminal;
                    if both_filled {
                        is_block_filled = true;
                    } else if reachable_blocks.contains(&frame.merge_block) {
                        if sealed_blocks.insert(frame.merge_block) {
                            builder.seal_block(frame.merge_block);
                        }
                        ensure_block_in_layout(&mut builder, frame.merge_block);
                        switch_to_block_tracking(
                            &mut builder,
                            frame.merge_block,
                            &mut is_block_filled,
                        );
                        // Materialize the merged value(s) for any `phi` ops by binding the
                        // merge-block parameters to their output variable names.
                        // Guard: skip if the merge block was already filled (can't emit defs).
                        if !is_block_filled && !frame.phi_ops.is_empty() {
                            for (idx, (out, _then_name, _else_name)) in
                                frame.phi_ops.iter().enumerate()
                            {
                                let param =
                                    frame.phi_params.get(idx).copied().unwrap_or_else(|| {
                                        panic!("phi param missing for {out} in {}", func_ir.name)
                                    });
                                def_var_named(&mut builder, &vars, out, param);
                            }
                            // Refcount tracking is name-based. A `phi` output is a new name for a
                            // value that came from one of the predecessor blocks. If we don't
                            // transfer tracking to the output name, the predecessor name can be
                            // decref'd at the phi boundary while the output is still live,
                            // leading to UAF/segfaults for object-valued if-expressions.
                            if let Some(tracked) = block_tracked_obj.get_mut(&frame.merge_block) {
                                let mut remove_names: BTreeSet<&str> = BTreeSet::new();
                                for (_out, then_name, else_name) in &frame.phi_ops {
                                    remove_names.insert(then_name.as_str());
                                    remove_names.insert(else_name.as_str());
                                }
                                tracked.retain(|name| !remove_names.contains(name.as_str()));
                                let mut present: BTreeSet<String> =
                                    tracked.iter().cloned().collect();
                                for (out, _then_name, _else_name) in &frame.phi_ops {
                                    if present.insert(out.clone()) {
                                        tracked.push(out.clone());
                                    }
                                }
                            }
                        }
                    } else {
                        is_block_filled = true;
                    }
                }
                "loop_start" => {
                    let indexed_loop_follows = func_ir
                        .ops
                        .get(op_idx + 1)
                        .is_some_and(|next| next.kind == "loop_index_start");
                    if indexed_loop_follows {
                        // Indexed loops are emitted as LOOP_START + LOOP_INDEX_START.
                        // LOOP_INDEX_START owns the loop frame and IV block param.
                        continue;
                    }
                    let loop_block = builder.create_block();
                    let body_block = builder.create_block();
                    let after_block = builder.create_block();
                    if !is_block_filled {
                        ensure_block_in_layout(&mut builder, loop_block);
                        reachable_blocks.insert(loop_block);
                        jump_block(&mut builder, loop_block, &[]);
                        switch_to_block_tracking(&mut builder, loop_block, &mut is_block_filled);
                    } else {
                        is_block_filled = true;
                    }
                    loop_stack.push(LoopFrame {
                        loop_block,
                        body_block,
                        after_block,
                        index_name: None,
                        next_index: None,
                    });
                    loop_depth += 1;
                }
                "loop_index_start" => {
                    let Some(out_name) = op.out else { continue; };
                    let loop_block = builder.create_block();
                    let body_block = builder.create_block();
                    let after_block = builder.create_block();
                    let idx_param = builder.append_block_param(loop_block, types::I64);
                    if !is_block_filled {
                        let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                        let start = var_get(&mut builder, &vars, &args[0])
                            .expect("Loop index start not found");
                        // Workaround for Cranelift 0.130 remove_constant_phis
                        // bug: when inside a nested loop the start value is
                        // derived from the outer loop's block param. The
                        // remove_constant_phis pass incorrectly tries to
                        // rewrite the inner loop header, panicking at
                        // "you cannot switch to a block which is already
                        // filled". Routing through a dedicated pre-header
                        // block isolates the entry edge so the pass does not
                        // see a single-source constant phi on the loop header.
                        if loop_depth > 0 {
                            let preheader = builder.create_block();
                            let ph_param = builder.append_block_param(preheader, types::I64);
                            ensure_block_in_layout(&mut builder, preheader);
                            reachable_blocks.insert(preheader);
                            jump_block(&mut builder, preheader, &[*start]);
                            switch_to_block_tracking(&mut builder, preheader, &mut is_block_filled);
                            builder.seal_block(preheader);
                            ensure_block_in_layout(&mut builder, loop_block);
                            reachable_blocks.insert(loop_block);
                            jump_block(&mut builder, loop_block, &[ph_param]);
                            switch_to_block_tracking(&mut builder, loop_block, &mut is_block_filled);
                        } else {
                            ensure_block_in_layout(&mut builder, loop_block);
                            reachable_blocks.insert(loop_block);
                            jump_block(&mut builder, loop_block, &[*start]);
                            switch_to_block_tracking(&mut builder, loop_block, &mut is_block_filled);
                        }
                    } else {
                        is_block_filled = true;
                    }
                    if reachable_blocks.contains(&loop_block) {
                        def_var_named(&mut builder, &vars, out_name.clone(), idx_param);
                    }
                    loop_stack.push(LoopFrame {
                        loop_block,
                        body_block,
                        after_block,
                        index_name: Some(out_name),
                        next_index: None,
                    });
                    loop_depth += 1;
                }
                "loop_break_if_true" => {
                    if loop_stack.is_empty() {
                        is_block_filled = true;
                    } else {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let cond =
                        var_get(&mut builder, &vars, &args[0]).expect("Loop break cond not found");
                    let frame = loop_stack.last().unwrap();
                    let current_block = builder
                        .current_block()
                        .expect("loop_break_if_true requires an active block");
                    let mut carry_obj_lb = block_tracked_obj.remove(&current_block).unwrap_or_default();
                    let tracked_obj_snapshot = drain_cleanup_tracked(&mut carry_obj_lb, &last_use, op_idx, None);
                    let mut carry_ptr_lb = block_tracked_ptr.remove(&current_block).unwrap_or_default();
                    let tracked_ptr_snapshot = drain_cleanup_tracked(&mut carry_ptr_lb, &last_use, op_idx, None);
                    // Fast path: extract bool payload directly for NaN-boxed
                    // booleans from fast_int comparisons (mirrors loop_break_if_false).
                    let prev_is_fast_bool = op_idx > 0 && {
                        let prev = &func_ir.ops[op_idx - 1];
                        prev.fast_int.unwrap_or(false)
                            && matches!(
                                prev.kind.as_str(),
                                "lt" | "le" | "gt" | "ge" | "eq" | "ne"
                            )
                    };
                    let cond_bool = if prev_is_fast_bool {
                        let one = builder.ins().iconst(types::I64, 1);
                        let payload = builder.ins().band(*cond, one);
                        builder.ins().icmp_imm(IntCC::NotEqual, payload, 0)
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_is_truthy", &[types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*cond]);
                        let truthy = builder.inst_results(call)[0];
                        builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0)
                    };
                    let cleanup_block = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(cleanup_block, current_block);
                    }
                    reachable_blocks.insert(cleanup_block);
                    reachable_blocks.insert(frame.body_block);
                    builder
                        .ins()
                        .brif(cond_bool, cleanup_block, &[], frame.body_block, &[]);
                    switch_to_block_tracking(&mut builder, cleanup_block, &mut is_block_filled);
                    builder.seal_block(cleanup_block);
                    for name in tracked_obj_snapshot {
                        let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                            panic!(
                                "Tracked obj var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                    for name in tracked_ptr_snapshot {
                        let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                            panic!(
                                "Tracked ptr var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                    reachable_blocks.insert(frame.after_block);
                    jump_block(&mut builder, frame.after_block, &[]);
                    switch_to_block_tracking(&mut builder, frame.body_block, &mut is_block_filled);
                    // Seal body_block now — its only predecessor is the brif above.
                    if sealed_blocks.insert(frame.body_block) {
                        builder.seal_block(frame.body_block);
                    }
                    propagate_tracked_to_branches(&mut block_tracked_obj, &[frame.body_block, frame.after_block], carry_obj_lb);
                    propagate_tracked_to_branches(&mut block_tracked_ptr, &[frame.body_block, frame.after_block], carry_ptr_lb);
                    }
                }
                "loop_break_if_false" => {
                    if loop_stack.is_empty() {
                        is_block_filled = true;
                    } else {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let cond =
                        var_get(&mut builder, &vars, &args[0]).expect("Loop break cond not found");
                    let frame = loop_stack.last().unwrap();
                    let current_block = builder
                        .current_block()
                        .expect("loop_break_if_false requires an active block");
                    let mut carry_obj_lb = block_tracked_obj.remove(&current_block).unwrap_or_default();
                    let tracked_obj_snapshot = drain_cleanup_tracked(&mut carry_obj_lb, &last_use, op_idx, None);
                    let mut carry_ptr_lb = block_tracked_ptr.remove(&current_block).unwrap_or_default();
                    let tracked_ptr_snapshot = drain_cleanup_tracked(&mut carry_ptr_lb, &last_use, op_idx, None);
                    // Fast path: when the condition is a NaN-boxed bool from a
                    // fast_int comparison (lt/le/gt/ge/eq/ne), extract the bool
                    // payload directly instead of calling molt_is_truthy.  This
                    // eliminates a runtime call per loop iteration AND avoids
                    // inserting extra Cranelift blocks between the loop header
                    // and body — keeping SSA variable propagation clean so the
                    // loop induction variable is correctly threaded through the
                    // back-edge.
                    let prev_is_fast_bool = op_idx > 0 && {
                        let prev = &func_ir.ops[op_idx - 1];
                        prev.fast_int.unwrap_or(false)
                            && matches!(
                                prev.kind.as_str(),
                                "lt" | "le" | "gt" | "ge" | "eq" | "ne"
                            )
                    };
                    let cond_bool = if prev_is_fast_bool {
                        // Condition is QNAN|TAG_BOOL|{0,1}: low bit is the bool.
                        let one = builder.ins().iconst(types::I64, 1);
                        let payload = builder.ins().band(*cond, one);
                        builder.ins().icmp_imm(IntCC::NotEqual, payload, 0)
                    } else {
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_is_truthy", &[types::I64], &[types::I64]);
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*cond]);
                        let truthy = builder.inst_results(call)[0];
                        builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0)
                    };
                    let cleanup_block = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(cleanup_block, current_block);
                    }
                    reachable_blocks.insert(frame.body_block);
                    reachable_blocks.insert(cleanup_block);
                    builder
                        .ins()
                        .brif(cond_bool, frame.body_block, &[], cleanup_block, &[]);
                    switch_to_block_tracking(&mut builder, cleanup_block, &mut is_block_filled);
                    builder.seal_block(cleanup_block);
                    for name in tracked_obj_snapshot {
                        let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                            panic!(
                                "Tracked obj var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                    for name in tracked_ptr_snapshot {
                        let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                            panic!(
                                "Tracked ptr var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                    reachable_blocks.insert(frame.after_block);
                    jump_block(&mut builder, frame.after_block, &[]);
                    switch_to_block_tracking(&mut builder, frame.body_block, &mut is_block_filled);
                    // Seal body_block now — its only predecessor is the brif
                    // above.  Early sealing helps Cranelift resolve SSA variables
                    // (especially the loop induction variable) immediately.
                    if sealed_blocks.insert(frame.body_block) {
                        builder.seal_block(frame.body_block);
                    }
                    propagate_tracked_to_branches(&mut block_tracked_obj, &[frame.body_block, frame.after_block], carry_obj_lb);
                    propagate_tracked_to_branches(&mut block_tracked_ptr, &[frame.body_block, frame.after_block], carry_ptr_lb);
                    }
                }
                "loop_break" => {
                    if loop_stack.is_empty() {
                        // break duplicated into an outer exception handler
                        // that sits after the loop boundary — treat as dead.
                        is_block_filled = true;
                    } else {
                    let frame = loop_stack.last().unwrap();
                    let current_block = builder
                        .current_block()
                        .expect("loop_break requires an active block");
                    if let Some(names) = block_tracked_obj.get_mut(&current_block) {
                        let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                        for name in cleanup {
                            // Use entry_vars (definition-time Value) for dec_ref,
                            // not var_get (current SSA Value). If the variable was
                            // redefined, var_get returns the WRONG object.
                            let val = entry_vars.get(&name).copied()
                                .or_else(|| var_get(&mut builder, &vars, &name).map(|v| *v));
                            let Some(val) = val else { continue; };
                            builder.ins().call(local_dec_ref_obj, &[val]);
                        }
                    }
                    if let Some(names) = block_tracked_ptr.get_mut(&current_block) {
                        let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                        for name in cleanup {
                            let val = entry_vars.get(&name).copied()
                                .or_else(|| var_get(&mut builder, &vars, &name).map(|v| *v));
                            let Some(val) = val else { continue; };
                            builder.ins().call(local_dec_ref_obj, &[val]);
                        }
                    }
                    reachable_blocks.insert(frame.after_block);
                    jump_block(&mut builder, frame.after_block, &[]);
                    is_block_filled = true;
                    }
                }
                "loop_index_next" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let next_idx =
                        var_get(&mut builder, &vars, &args[0]).expect("Loop index next not found");
                    if loop_stack.is_empty() {
                        // The loop_index_next op appears outside the loop
                        // boundary — this happens when `continue` inside a
                        // nested try/except is duplicated into an outer
                        // exception handler path.  Treat as unreachable.
                        let Some(out_name) = op.out else { continue; };
                        def_var_named(&mut builder, &vars, out_name, *next_idx);
                    } else {
                        let frame = loop_stack.last_mut().unwrap();
                        frame.next_index = Some(*next_idx);
                        let Some(out_name) = op.out else { continue; };
                        def_var_named(&mut builder, &vars, out_name, *next_idx);
                    }
                }
                "loop_continue" => {
                    if loop_stack.is_empty() {
                        // Same as loop_index_next: the continue was
                        // duplicated into an outer exception handler that
                        // sits after the loop's END_LOOP.  Mark the block
                        // as filled so subsequent ops are dead code.
                        is_block_filled = true;
                    } else {
                    let frame = loop_stack.last_mut().unwrap();
                    let current_block = builder
                        .current_block()
                        .expect("loop_continue requires an active block");
                    if let Some(names) = block_tracked_obj.get_mut(&current_block) {
                        let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                        for name in cleanup {
                            // Use entry_vars (definition-time Value) for dec_ref,
                            // not var_get (current SSA Value). If the variable was
                            // redefined, var_get returns the WRONG object.
                            let val = entry_vars.get(&name).copied()
                                .or_else(|| var_get(&mut builder, &vars, &name).map(|v| *v));
                            let Some(val) = val else { continue; };
                            builder.ins().call(local_dec_ref_obj, &[val]);
                        }
                    }
                    if let Some(names) = block_tracked_ptr.get_mut(&current_block) {
                        let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                        for name in cleanup {
                            let val = entry_vars.get(&name).copied()
                                .or_else(|| var_get(&mut builder, &vars, &name).map(|v| *v));
                            let Some(val) = val else { continue; };
                            builder.ins().call(local_dec_ref_obj, &[val]);
                        }
                    }
                    reachable_blocks.insert(frame.loop_block);
                    if let Some(next_idx) = frame.next_index.take() {
                        jump_block(&mut builder, frame.loop_block, &[next_idx]);
                    } else if let Some(name) = frame.index_name.as_ref() {
                        let current_idx =
                            var_get(&mut builder, &vars, name).expect("Loop index not found");
                        jump_block(&mut builder, frame.loop_block, &[*current_idx]);
                    } else {
                        jump_block(&mut builder, frame.loop_block, &[]);
                    }
                    is_block_filled = true;
                    }
                }
                "loop_end" => {
                    if loop_stack.is_empty() {
                        // Orphan loop_end from a duplicated exception
                        // handler path — skip silently.
                    } else {
                    let mut frame = loop_stack.pop().unwrap();
                    loop_depth -= 1;
                    if !is_block_filled {
                        ensure_block_in_layout(&mut builder, frame.loop_block);
                        reachable_blocks.insert(frame.loop_block);
                        if let Some(next_idx) = frame.next_index.take() {
                            jump_block(&mut builder, frame.loop_block, &[next_idx]);
                        } else if let Some(name) = frame.index_name.as_ref() {
                            let current_idx =
                                var_get(&mut builder, &vars, name).expect("Loop index not found");
                            jump_block(&mut builder, frame.loop_block, &[*current_idx]);
                        } else {
                            jump_block(&mut builder, frame.loop_block, &[]);
                        }
                    }
                    if builder.func.layout.is_block_inserted(frame.loop_block) {
                        builder.seal_block(frame.loop_block);
                    }
                    if reachable_blocks.contains(&frame.after_block) {
                        ensure_block_in_layout(&mut builder, frame.after_block);
                        switch_to_block_tracking(
                            &mut builder,
                            frame.after_block,
                            &mut is_block_filled,
                        );
                        if builder.func.layout.is_block_inserted(frame.after_block) {
                            builder.seal_block(frame.after_block);
                        }
                    } else {
                        is_block_filled = true;
                    }
                    }
                }
                "alloc" => {
                    let size = op.value.unwrap_or(0);
                    let iconst = builder.ins().iconst(types::I64, size);

                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_alloc", &[types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst]);
                    let res = builder.inst_results(call)[0];
                    let Some(out_name) = op.out else { continue; };
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "alloc_class" => {
                    let size = op.value.unwrap_or(0);
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let iconst = builder.ins().iconst(types::I64, size);

                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_alloc_class", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    let Some(out_name) = op.out else { continue; };
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "alloc_class_trusted" => {
                    let size = op.value.unwrap_or(0);
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let iconst = builder.ins().iconst(types::I64, size);

                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_alloc_class_trusted", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    let Some(out_name) = op.out else { continue; };
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "alloc_class_static" => {
                    let size = op.value.unwrap_or(0);
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let iconst = builder.ins().iconst(types::I64, size);

                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_alloc_class_static", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    let Some(out_name) = op.out else { continue; };
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "alloc_task" => {
                    let closure_size = op.value.unwrap_or(0);
                    let task_kind = op.task_kind.as_deref().unwrap_or("future");
                    let (kind_bits, payload_base) = match task_kind {
                        "generator" => (TASK_KIND_GENERATOR, GENERATOR_CONTROL_BYTES),
                        "future" => (TASK_KIND_FUTURE, 0),
                        "coroutine" => (TASK_KIND_COROUTINE, 0),
                        _ => panic!("unknown task kind: {task_kind}"),
                    };
                    let size = builder.ins().iconst(types::I64, closure_size);

                    let Some(poll_func_name) = op.s_value.as_ref() else { continue; };
                    let mut poll_sig = self.module.make_signature();
                    poll_sig.params.push(AbiParam::new(types::I64));
                    poll_sig.returns.push(AbiParam::new(types::I64));

                    let poll_linkage = if defined_functions.contains(poll_func_name.as_str()) {
                        Linkage::Export
                    } else {
                        Linkage::Import
                    };
                    let poll_func_id = self
                        .module
                        .declare_function(poll_func_name, poll_linkage, &poll_sig)
                        .unwrap();
                    let poll_func_ref =
                        self.module.declare_func_in_func(poll_func_id, builder.func);
                    let poll_addr = builder.ins().func_addr(types::I64, poll_func_ref);

                    let task_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_task_new", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let task_local = self.module.declare_func_in_func(task_callee, builder.func);
                    let kind_val = builder.ins().iconst(types::I64, kind_bits);
                    let call = builder.ins().call(task_local, &[poll_addr, size, kind_val]);
                    let obj = builder.inst_results(call)[0];
                    let obj_ptr = unbox_ptr_value(&mut builder, obj, &nbc);
                    if let Some(args_names) = &op.args {
                        for (i, name) in args_names.iter().enumerate() {
                            let arg_val = var_get(&mut builder, &vars, name)
                                .expect("Arg not found for alloc_task");
                            let offset = payload_base + (i * 8) as i32;
                            builder
                                .ins()
                                .store(MemFlags::trusted(), *arg_val, obj_ptr, offset);
                            emit_maybe_ref_adjust_v2(&mut builder, *arg_val, local_inc_ref_obj, &nbc);
                        }
                    }
                    if matches!(task_kind, "future" | "coroutine") {
                        let get_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_cancel_token_get_current", &[], &[types::I64]);
                        let get_local = self.module.declare_func_in_func(get_callee, builder.func);
                        let get_call = builder.ins().call(get_local, &[]);
                        let current_token = builder.inst_results(get_call)[0];

                        let reg_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_task_register_token_owned", &[types::I64, types::I64], &[types::I64]);
                        let reg_local = self.module.declare_func_in_func(reg_callee, builder.func);
                        builder.ins().call(reg_local, &[obj, current_token]);
                    }

                    output_is_ptr = false;
                    let Some(out_name) = op.out else { continue; };
                    def_var_named(&mut builder, &vars, out_name, obj);
                }
                "store" => {
                    let local_profile_struct =
                        local_profile_struct.expect("store lowering requires profile import");
                    let profile_enabled_val =
                        profile_enabled_val.expect("store lowering requires profile flag");
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Value not found");
                    let offset = op.value.unwrap_or(0) as i32;
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj, &nbc);
                    let profile_block = builder.create_block();
                    let profile_cont = builder.create_block();
                    if let Some(current_block) = builder.current_block() {
                        builder.insert_block_after(profile_block, current_block);
                        builder.insert_block_after(profile_cont, profile_block);
                    }
                    let profile_bool =
                        builder
                            .ins()
                            .icmp_imm(IntCC::NotEqual, profile_enabled_val, 0);
                    builder
                        .ins()
                        .brif(profile_bool, profile_block, &[], profile_cont, &[]);
                    builder.switch_to_block(profile_block);
                    builder.seal_block(profile_block);
                    builder.ins().call(local_profile_struct, &[]);
                    jump_block(&mut builder, profile_cont, &[]);
                    builder.switch_to_block(profile_cont);
                    builder.seal_block(profile_cont);
                    let offset_bits = builder.ins().iconst(types::I64, i64::from(offset));
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_object_field_set_ptr", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[obj_ptr, offset_bits, *val]);
                    if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, out_name.clone(), res);
                    }
                }
                "store_init" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Value not found");
                    let offset = op.value.unwrap_or(0) as i32;
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj, &nbc);
                    let offset_bits = builder.ins().iconst(types::I64, i64::from(offset));
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_object_field_init_ptr", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[obj_ptr, offset_bits, *val]);
                    if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, out_name.clone(), res);
                    }
                }
                "load" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let offset = op.value.unwrap_or(0) as i32;
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj, &nbc);
                    let res = builder
                        .ins()
                        .load(types::I64, MemFlags::trusted(), obj_ptr, offset);
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj, &nbc);
                    let Some(out_name) = op.out else { continue; };
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "closure_load" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap_or(0));
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj, &nbc);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_closure_load", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[obj_ptr, offset]);
                    let res = builder.inst_results(call)[0];
                    let Some(out_name) = op.out else { continue; };
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "closure_store" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Value not found");
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap_or(0));
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj, &nbc);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_closure_store", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[obj_ptr, offset, *val]);
                    if let Some(out_name) = op.out {
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, out_name, res);
                    }
                }
                "guarded_load" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let offset = op.value.unwrap_or(0) as i32;
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj, &nbc);
                    let res = builder
                        .ins()
                        .load(types::I64, MemFlags::trusted(), obj_ptr, offset);
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj, &nbc);
                    let Some(out_name) = op.out else { continue; };
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "guarded_field_get" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj, &nbc);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let expected_version =
                        var_get(&mut builder, &vars, &args[2]).expect("Expected version not found");
                    let Some(attr_name) = op.s_value.as_ref() else { continue; };
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap_or(0));
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_guarded_field_get_ptr", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[
                            obj_ptr,
                            *class_bits,
                            *expected_version,
                            offset,
                            attr_ptr,
                            attr_len,
                        ],
                    );
                    let res = builder.inst_results(call)[0];
                    let Some(out_name) = op.out else { continue; };
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "guarded_field_set" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj, &nbc);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let expected_version =
                        var_get(&mut builder, &vars, &args[2]).expect("Expected version not found");
                    let val = var_get(&mut builder, &vars, &args[3]).expect("Value not found");
                    let Some(attr_name) = op.s_value.as_ref() else { continue; };
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap_or(0));
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_guarded_field_set_ptr", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[
                            obj_ptr,
                            *class_bits,
                            *expected_version,
                            offset,
                            *val,
                            attr_ptr,
                            attr_len,
                        ],
                    );
                    if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, out_name.clone(), res);
                    }
                }
                "guarded_field_init" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj, &nbc);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let expected_version =
                        var_get(&mut builder, &vars, &args[2]).expect("Expected version not found");
                    let val = var_get(&mut builder, &vars, &args[3]).expect("Value not found");
                    let Some(attr_name) = op.s_value.as_ref() else { continue; };
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap_or(0));
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_guarded_field_init_ptr", &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[
                            obj_ptr,
                            *class_bits,
                            *expected_version,
                            offset,
                            *val,
                            attr_ptr,
                            attr_len,
                        ],
                    );
                    if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, out_name.clone(), res);
                    }
                }
                "guard_type" | "guard_tag" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val =
                        var_get(&mut builder, &vars, &args[0]).expect("Guard value not found");
                    let expected = var_get(&mut builder, &vars, &args[1])
                        .expect("Guard expected tag not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_guard_type", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*val, *expected]);
                }
                "guard_layout" | "guard_dict_shape" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Guard object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj, &nbc);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Guard class not found");
                    let expected_version =
                        var_get(&mut builder, &vars, &args[2]).expect("Guard version not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_guard_layout_ptr", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[obj_ptr, *class_bits, *expected_version]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "get_attr_generic_ptr" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj, &nbc);
                    let Some(attr_name) = op.s_value.as_ref() else { continue; };
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);

                    let res = if let Some(ic_idx) = op.ic_index {
                        // Split-phase IC: fast GIL-free probe, then slow path on miss.
                        //
                        // Phase 1: molt_ic_probe_fast(obj_ptr, ic_index) → hit or 0
                        // Phase 2 (miss only): molt_getattr_ic_slow(obj_ptr, attr, len, ic_index)
                        //
                        // The raw ic_index is passed as a plain i64 — NOT NaN-boxed —
                        // because the runtime treats it as a direct table index.
                        let ic_raw = builder.ins().iconst(types::I64, ic_idx);

                        // --- Declare molt_ic_probe_fast(obj_ptr, ic_index) -> i64 ---
                        let probe_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_ic_probe_fast", &[types::I64, types::I64], &[types::I64]);
                        let probe_local =
                            self.module.declare_func_in_func(probe_callee, builder.func);

                        // --- Declare molt_getattr_ic_slow(obj_ptr, attr, len, ic_index) -> i64 ---
                        let slow_callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_getattr_ic_slow", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                        let slow_local =
                            self.module.declare_func_in_func(slow_callee, builder.func);

                        // --- Emit: probe_result = molt_ic_probe_fast(obj_ptr, ic_raw) ---
                        let probe_call =
                            builder.ins().call(probe_local, &[obj_ptr, ic_raw]);
                        let probe_result = builder.inst_results(probe_call)[0];

                        // --- Branch: hit (probe_result != 0) vs miss ---
                        let hit_block = builder.create_block();
                        let miss_block = builder.create_block();
                        builder.set_cold_block(miss_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let zero = builder.ins().iconst(types::I64, 0);
                        let is_hit = builder.ins().icmp(
                            IntCC::NotEqual,
                            probe_result,
                            zero,
                        );
                        builder
                            .ins()
                            .brif(is_hit, hit_block, &[], miss_block, &[]);

                        // --- Hit block: probe returned an owned reference ---
                        builder.switch_to_block(hit_block);
                        builder.seal_block(hit_block);
                        jump_block(&mut builder, merge_block, &[probe_result]);

                        // --- Miss block: full resolution via slow path ---
                        builder.switch_to_block(miss_block);
                        builder.seal_block(miss_block);
                        let slow_call = builder.ins().call(
                            slow_local,
                            &[obj_ptr, attr_ptr, attr_len, ic_raw],
                        );
                        let slow_result = builder.inst_results(slow_call)[0];
                        // Slow path returns a borrowed reference; inc_ref to own it.
                        emit_maybe_ref_adjust_v2(
                            &mut builder,
                            slow_result,
                            local_inc_ref_obj,
                            &nbc,
                        );
                        jump_block(&mut builder, merge_block, &[slow_result]);

                        // --- Merge ---
                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    } else {
                        // Legacy path: no IC index available.
                        let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_get_attr_ptr", &[types::I64, types::I64, types::I64], &[types::I64]);
                        let local_callee =
                            self.module.declare_func_in_func(callee, builder.func);
                        let call = builder
                            .ins()
                            .call(local_callee, &[obj_ptr, attr_ptr, attr_len]);
                        let slow_res = builder.inst_results(call)[0];
                        // Attribute lookup may return borrowed values from object/class internals.
                        // Normalize to an owned reference so last-use decref remains safe.
                        emit_maybe_ref_adjust_v2(&mut builder, slow_res, local_inc_ref_obj, &nbc);
                        slow_res
                    };
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "get_attr_generic_obj" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let Some(attr_name) = op.s_value.as_ref() else { continue; };
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_get_attr_object_ic", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let site_bits = builder.ins().iconst(
                        types::I64,
                        box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            "get_attr_generic_obj",
                        )),
                    );
                    let call = builder
                        .ins()
                        .call(local_callee, &[*obj, attr_ptr, attr_len, site_bits]);
                    let res = builder.inst_results(call)[0];
                    // `molt_get_attr_object_ic` delegates to `molt_get_attr_name`, which can
                    // hand back borrowed values on fast paths. Own the result here.
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj, &nbc);
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "get_attr_special_obj" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let Some(attr_name) = op.s_value.as_ref() else { continue; };
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_get_attr_special", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*obj, attr_ptr, attr_len]);
                    let res = builder.inst_results(call)[0];
                    // Keep attribute result ownership consistent across all get-attr ops.
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj, &nbc);
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "get_attr_name" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = var_get(&mut builder, &vars, &args[1]).expect("Attr name not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_get_attr_name", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name]);
                    let res = builder.inst_results(call)[0];
                    // Attribute lookup returns a borrowed reference from object internals/dicts in
                    // some fast paths. Convert it to an owned reference so lifetime tracking can
                    // safely decref at last use without corrupting dict-owned values.
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj, &nbc);
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "get_attr_name_default" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = var_get(&mut builder, &vars, &args[1]).expect("Attr name not found");
                    let default =
                        var_get(&mut builder, &vars, &args[2]).expect("Attr default not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_get_attr_name_default", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name, *default]);
                    let res = builder.inst_results(call)[0];
                    // See `get_attr_name` above: ensure the returned value is owned.
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj, &nbc);
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "has_attr_name" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = var_get(&mut builder, &vars, &args[1]).expect("Attr name not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_has_attr_name", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "set_attr_name" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = var_get(&mut builder, &vars, &args[1]).expect("Attr name not found");
                    let val = var_get(&mut builder, &vars, &args[2]).expect("Attr value not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_set_attr_name", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "set_attr_generic_ptr" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj, &nbc);
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Attr value not found");
                    let Some(attr_name) = op.s_value.as_ref() else { continue; };
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_set_attr_ptr", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[obj_ptr, attr_ptr, attr_len, *val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "set_attr_generic_obj" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Attr value not found");
                    let Some(attr_name) = op.s_value.as_ref() else { continue; };
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_set_attr_object", &[types::I64, types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*obj, attr_ptr, attr_len, *val]);
                    if let Some(out_name) = op.out {
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, out_name, res);
                    }
                }
                "del_attr_generic_ptr" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj, &nbc);
                    let Some(attr_name) = op.s_value.as_ref() else { continue; };
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_del_attr_ptr", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[obj_ptr, attr_ptr, attr_len]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "del_attr_generic_obj" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let Some(attr_name) = op.s_value.as_ref() else { continue; };
                    let data_id = self
                        .module
                        .declare_data(
                            &format!("attr_{}_{}", func_ir.name, op_idx),
                            Linkage::Export,
                            false,
                            false,
                        )
                        .unwrap();
                    let mut data_ctx = DataDescription::new();
                    data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
                    self.module.define_data(data_id, &data_ctx).unwrap();

                    let global_ptr = self.module.declare_data_in_func(data_id, builder.func);
                    let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
                    let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_del_attr_object", &[types::I64, types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*obj, attr_ptr, attr_len]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "del_attr_name" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = var_get(&mut builder, &vars, &args[1]).expect("Attr name not found");
                    let callee = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_del_attr_name", &[types::I64, types::I64], &[types::I64]);
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out { def_var_named(&mut builder, &vars, out__, res); }
                }
                "ret" => {
                    if std::env::var("MOLT_DEBUG_RET_CLEANUP").as_deref() == Ok("1")
                        && std::env::var("MOLT_DEBUG_FUNC_FILTER")
                            .ok()
                            .map_or(true, |f| func_ir.name.contains(&f))
                    {
                        eprintln!(
                            "debug ret cleanup func={} op_idx={} ret_var={:?} tracked_obj_vars_len={} tracked_vars_len={}",
                            func_ir.name,
                            op_idx,
                            op.var.as_deref(),
                            tracked_obj_vars.len(),
                            tracked_vars.len(),
                        );
                        if !tracked_obj_vars.is_empty() {
                            eprintln!("debug ret cleanup tracked_obj_vars={:?}", tracked_obj_vars);
                        }
                        if !tracked_vars.is_empty() {
                            eprintln!("debug ret cleanup tracked_vars={:?}", tracked_vars);
                        }
                    }
                    let Some(var_name) = op.var.as_ref() else {
                        if let Some(block) = builder.current_block() {
                            // Function return: fully drain per-block tracked values.
                            if let Some(names) = block_tracked_obj.remove(&block) {
                                for name in names {
                                    let val =
                                        var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                            panic!(
                                                "Tracked obj var not found in {} op {}: {}",
                                                func_ir.name, op_idx, name
                                            )
                                        });
                                    builder.ins().call(local_dec_ref_obj, &[*val]);
                                }
                            }
                            if let Some(names) = block_tracked_ptr.remove(&block) {
                                for name in names {
                                    let val =
                                        var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                            panic!(
                                                "Tracked ptr var not found in {} op {}: {}",
                                                func_ir.name, op_idx, name
                                            )
                                        });
                                    builder.ins().call(local_dec_ref_obj, &[*val]);
                                }
                            }
                        }
                        for name in &tracked_vars {
                            if let Some(val) = var_get(&mut builder, &vars, name) {
                                builder.ins().call(local_dec_ref_obj, &[*val]);
                            }
                        }
                        for name in &tracked_obj_vars {
                            if let Some(val) = var_get(&mut builder, &vars, name) {
                                builder.ins().call(local_dec_ref_obj, &[*val]);
                            }
                        }
                        reachable_blocks.insert(master_return_block);
                        if has_ret {
                            let none_bits = builder.ins().iconst(types::I64, box_none());
                            jump_block(&mut builder, master_return_block, &[none_bits]);
                        } else {
                            jump_block(&mut builder, master_return_block, &[]);
                        }
                        is_block_filled = true;
                        continue;
                    };
                    let ret_val =
                        *var_get(&mut builder, &vars, var_name).expect("Return variable not found");
                    if let Some(block) = builder.current_block() {
                        // Function return: fully drain per-block tracked values (except return).
                        if let Some(names) = block_tracked_obj.remove(&block) {
                            for name in names {
                                if name == *var_name {
                                    continue;
                                }
                                let val = entry_vars.get(&name).copied()
                                    .or_else(|| var_get(&mut builder, &vars, &name).map(|v| *v));
                                let Some(val) = val else { continue; };
                                builder.ins().call(local_dec_ref_obj, &[val]);
                            }
                        }
                        if let Some(names) = block_tracked_ptr.remove(&block) {
                            for name in names {
                                if name == *var_name {
                                    continue;
                                }
                                let val = entry_vars.get(&name).copied()
                                    .or_else(|| var_get(&mut builder, &vars, &name).map(|v| *v));
                                let Some(val) = val else { continue; };
                                builder.ins().call(local_dec_ref_obj, &[val]);
                            }
                        }
                    }
                    tracked_vars.retain(|v| v != var_name);
                    tracked_obj_vars.retain(|v| v != var_name);
                    tracked_vars_set.remove(var_name);
                    tracked_obj_vars_set.remove(var_name);
                    for name in &tracked_vars {
                        let val = entry_vars.get(name).copied()
                            .or_else(|| var_get(&mut builder, &vars, name).map(|v| *v));
                        if let Some(val) = val {
                            builder.ins().call(local_dec_ref_obj, &[val]);
                        }
                    }
                    for name in &tracked_obj_vars {
                        let val = entry_vars.get(name).copied()
                            .or_else(|| var_get(&mut builder, &vars, name).map(|v| *v));
                        if let Some(val) = val {
                            builder.ins().call(local_dec_ref_obj, &[val]);
                        }
                    }
                    reachable_blocks.insert(master_return_block);
                    if has_ret {
                        jump_block(&mut builder, master_return_block, &[ret_val]);
                    } else {
                        jump_block(&mut builder, master_return_block, &[]);
                    }
                    is_block_filled = true;
                }
                "ret_void" => {
                    if let Some(block) = builder.current_block() {
                        // Function return: fully drain per-block tracked values.
                        if let Some(names) = block_tracked_obj.remove(&block) {
                            for name in names {
                                let val =
                                    var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                        panic!(
                                            "Tracked obj var not found in {} op {}: {}",
                                            func_ir.name, op_idx, name
                                        )
                                    });
                                builder.ins().call(local_dec_ref_obj, &[*val]);
                            }
                        }
                        if let Some(names) = block_tracked_ptr.remove(&block) {
                            for name in names {
                                let val =
                                    var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                        panic!(
                                            "Tracked ptr var not found in {} op {}: {}",
                                            func_ir.name, op_idx, name
                                        )
                                    });
                                builder.ins().call(local_dec_ref_obj, &[*val]);
                            }
                        }
                    }
                    for name in &tracked_vars {
                        if let Some(val) = entry_vars.get(name) {
                            builder.ins().call(local_dec_ref_obj, &[*val]);
                        }
                    }
                    for name in &tracked_obj_vars {
                        if let Some(val) = entry_vars.get(name) {
                            builder.ins().call(local_dec_ref_obj, &[*val]);
                        }
                    }
                    reachable_blocks.insert(master_return_block);
                    if has_ret {
                        let none_bits = builder.ins().iconst(types::I64, box_none());
                        jump_block(&mut builder, master_return_block, &[none_bits]);
                    } else {
                        jump_block(&mut builder, master_return_block, &[]);
                    }
                    is_block_filled = true;
                }
                "jump" => {
                    let target_id = op.value.unwrap_or(0);
                    let target_block = state_blocks[&target_id];
                    if let Some(block) = builder.current_block() {
                        let mut carry_obj = block_tracked_obj.remove(&block).unwrap_or_default();
                        let cleanup =
                            drain_cleanup_tracked(&mut carry_obj, &last_use, op_idx, None);
                        for name in cleanup {
                            // Use entry_vars (definition-time Value) for dec_ref,
                            // not var_get (current SSA Value). If the variable was
                            // redefined, var_get returns the WRONG object.
                            let val = entry_vars.get(&name).copied()
                                .or_else(|| var_get(&mut builder, &vars, &name).map(|v| *v));
                            let Some(val) = val else { continue; };
                            builder.ins().call(local_dec_ref_obj, &[val]);
                        }
                        if !carry_obj.is_empty() {
                            extend_unique_tracked(
                                block_tracked_obj.entry(target_block).or_default(),
                                carry_obj,
                            );
                        }

                        let mut carry_ptr = block_tracked_ptr.remove(&block).unwrap_or_default();
                        let cleanup =
                            drain_cleanup_tracked(&mut carry_ptr, &last_use, op_idx, None);
                        for name in cleanup {
                            let val = entry_vars.get(&name).copied()
                                .or_else(|| var_get(&mut builder, &vars, &name).map(|v| *v));
                            let Some(val) = val else { continue; };
                            builder.ins().call(local_dec_ref_obj, &[val]);
                        }
                        if !carry_ptr.is_empty() {
                            extend_unique_tracked(
                                block_tracked_ptr.entry(target_block).or_default(),
                                carry_ptr,
                            );
                        }
                    }
                    reachable_blocks.insert(target_block);
                    jump_block(&mut builder, target_block, &[]);
                    is_block_filled = true;
                }
                "br_if" => {
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let cond = var_get(&mut builder, &vars, &args[0]).expect("Cond not found");
                    let target_id = op.value.unwrap_or(0);
                    let target_block = state_blocks[&target_id];
                    let origin_block = builder
                        .current_block()
                        .expect("br_if requires an active block");

                    let fallthrough_block = builder.create_block();
                    // cond is NaN-boxed — must call molt_is_truthy to extract
                    // the boolean. NaN-boxed False is 0x7ffa000000000000 (nonzero),
                    // so a raw icmp_imm(!=0) always evaluates true.
                    let truthy_fn = Self::import_func_id_split(&mut self.module, &mut self.import_ids, "molt_is_truthy", &[types::I64], &[types::I64]);
                    let truthy_ref = self.module.declare_func_in_func(truthy_fn, builder.func);
                    let truthy_call = builder.ins().call(truthy_ref, &[*cond]);
                    let truthy_val = builder.inst_results(truthy_call)[0];
                    let cond_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy_val, 0);

                    reachable_blocks.insert(target_block);
                    reachable_blocks.insert(fallthrough_block);
                    // br_if terminates the current block and can transfer control to either
                    // successor. Carry all live tracked values into both.
                    let mut carry_obj = block_tracked_obj.remove(&origin_block).unwrap_or_default();
                    let cleanup = drain_cleanup_tracked(&mut carry_obj, &last_use, op_idx, None);
                    for name in cleanup {
                        let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                            panic!(
                                "Tracked obj var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                    if !carry_obj.is_empty() {
                        extend_unique_tracked(
                            block_tracked_obj.entry(target_block).or_default(),
                            carry_obj.clone(),
                        );
                        extend_unique_tracked(
                            block_tracked_obj.entry(fallthrough_block).or_default(),
                            carry_obj.clone(),
                        );
                    }
                    let mut carry_ptr = block_tracked_ptr.remove(&origin_block).unwrap_or_default();
                    let cleanup = drain_cleanup_tracked(&mut carry_ptr, &last_use, op_idx, None);
                    for name in cleanup {
                        let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                            panic!(
                                "Tracked ptr var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                    if !carry_ptr.is_empty() {
                        extend_unique_tracked(
                            block_tracked_ptr.entry(target_block).or_default(),
                            carry_ptr.clone(),
                        );
                        extend_unique_tracked(
                            block_tracked_ptr.entry(fallthrough_block).or_default(),
                            carry_ptr.clone(),
                        );
                    }
                    builder
                        .ins()
                        .brif(cond_bool, target_block, &[], fallthrough_block, &[]);
                    switch_to_block_tracking(&mut builder, fallthrough_block, &mut is_block_filled);
                    builder.seal_block(fallthrough_block);
                }
                "label" | "state_label" => {
                    let label_id = op.value.unwrap_or(0);
                    let block = state_blocks[&label_id];
                    let is_function_exception_label = Some(label_id) == function_exception_label_id;

                    // Prevent normal fallthrough into the function-level exception handler.
                    if is_function_exception_label && !is_block_filled {
                        reachable_blocks.insert(master_return_block);
                        if has_ret {
                            let zero = builder.ins().iconst(types::I64, 0);
                            jump_block(&mut builder, master_return_block, &[zero]);
                        } else {
                            jump_block(&mut builder, master_return_block, &[]);
                        }
                        is_block_filled = true;
                    }

                    if is_function_exception_label {
                        // Exception handlers are cold — move them out of the
                        // hot execution path for better i-cache/branch behavior.
                        builder.set_cold_block(block);
                        ensure_block_in_layout(&mut builder, block);
                        reachable_blocks.insert(block);
                        switch_to_block_tracking(&mut builder, block, &mut is_block_filled);
                    } else if !is_block_filled {
                        reachable_blocks.insert(block);
                        jump_block(&mut builder, block, &[]);
                        switch_to_block_tracking(&mut builder, block, &mut is_block_filled);
                    } else if reachable_blocks.contains(&block) {
                        switch_to_block_tracking(&mut builder, block, &mut is_block_filled);
                    } else {
                        is_block_filled = true;
                    }
                }
                "phi" => {}
                // TIR round-trip variable ops — wire SSA values between blocks
                "store_var" => {
                    // Store a value into a named variable (block arg passing)
                    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                    let val = var_get(&mut builder, &vars, &args[0]).expect("store_var: src not found");
                    if let Some(ref var_name) = op.var {
                        def_var_named(&mut builder, &vars, var_name, *val);
                    } else if let Some(ref out_name) = op.out {
                        def_var_named(&mut builder, &vars, out_name, *val);
                    }
                }
                "load_var" | "copy_var" => {
                    // Load a named variable into an output (block arg receiving / copy)
                    if let Some(ref var_name) = op.var {
                        let val = var_get(&mut builder, &vars, var_name).expect("load_var: var not found");
                        if let Some(ref out_name) = op.out {
                            def_var_named(&mut builder, &vars, out_name, *val);
                        }
                    } else if let Some(ref args) = op.args {
                        if !args.is_empty() {
                            let val = var_get(&mut builder, &vars, &args[0]).expect("copy_var: src not found");
                            if let Some(ref out_name) = op.out {
                                def_var_named(&mut builder, &vars, out_name, *val);
                            }
                        }
                    }
                }
                "load_param" => {
                    // TIR emits load_param for function parameters — map param index
                    // to the corresponding block param value
                    let param_idx = op.value.unwrap_or(0) as usize;
                    if let Some(ref out_name) = op.out {
                        let entry_block = builder.func.layout.entry_block().unwrap();
                        let param_val = {
                            let params = builder.func.dfg.block_params(entry_block);
                            if param_idx < params.len() { Some(params[param_idx]) } else { None }
                        };
                        if let Some(val) = param_val {
                            def_var_named(&mut builder, &vars, out_name, val);
                        }
                    }
                }
                _ => {}
            }

            // IMPORTANT: entry-tracked cleanup must be control-flow safe.
            //
            // `tracked_obj_vars`/`tracked_vars` are populated only for values defined in the
            // entry block, but this loop walks IR ops in a linear order while switching across
            // blocks for `if`/`else`/loops. Draining the entry-tracked lists while we are
            // emitting code for a non-entry block can incorrectly place the decref only on one
            // branch (for example the `then` side of an `if`), causing leaks on the other path.
            //
            // We therefore only drain entry-tracked cleanup while still emitting the entry block.
            // Values whose "last use" happens exclusively in a non-entry block remain live until
            // the function-level return cleanup, which is emitted on all paths.
            if !is_block_filled && loop_depth == 0 && builder.current_block() == Some(entry_block) {
                let cleanup = drain_cleanup_entry_tracked(
                    &mut tracked_obj_vars,
                    &mut entry_vars,
                    &last_use,
                    op_idx,
                );
                for val in cleanup {
                    builder.ins().call(local_dec_ref_obj, &[val]);
                }
                let cleanup =
                    drain_cleanup_entry_tracked(&mut tracked_vars, &mut entry_vars, &last_use, op_idx);
                for val in cleanup {
                    // Use dec_ref_obj (NaN-box aware) instead of dec_ref (raw ptr).
                    // entry_vars always stores NaN-boxed bits, not raw pointers,
                    // so we must use the variant that checks the tag before
                    // dereferencing.  Using raw dec_ref here would SIGSEGV for
                    // any non-pointer NaN-boxed value (floats, inline ints, etc.).
                    builder.ins().call(local_dec_ref_obj, &[val]);
                }
            }

            if let Some(name) = out_name.as_ref()
                && name != "none"
                && let Some(block) = builder.current_block()
                // RC coalescing: skip tracking for variables whose dec_ref
                // was elided because the matching inc_ref was also elided.
                && !rc_skip_dec.contains(name.as_str())
            {
                if block == entry_block && loop_depth == 0 {
                    if output_is_ptr {
                        if tracked_vars_set.insert(name.to_string()) {
                            tracked_vars.push(name.clone());
                        }
                    } else {
                        if tracked_obj_vars_set.insert(name.to_string()) {
                            tracked_obj_vars.push(name.clone());
                        }
                    }
                    if let Some(val) = var_get(&mut builder, &vars, name) {
                        entry_vars.insert(name.clone(), *val);
                    }
                } else if output_is_ptr {
                    block_tracked_ptr
                        .entry(block)
                        .or_default()
                        .push(name.to_string());
                } else {
                    block_tracked_obj
                        .entry(block)
                        .or_default()
                        .push(name.to_string());
                }
            }
        }

        // Finalize Master Return Block
        if !is_block_filled {
            // Both tracked_vars and tracked_obj_vars store NaN-boxed bits in
            // entry_vars, so always use dec_ref_obj (NaN-box aware) for cleanup.
            // Using raw dec_ref on NaN-boxed bits causes SIGSEGV for non-pointer
            // values (floats from abs/round, inline ints, etc.).
            for name in &tracked_vars {
                if let Some(val) = entry_vars.get(name) {
                    builder.ins().call(local_dec_ref_obj, &[*val]);
                }
            }
            for name in &tracked_obj_vars {
                if let Some(val) = entry_vars.get(name) {
                    builder.ins().call(local_dec_ref_obj, &[*val]);
                }
            }
            if has_ret {
                let zero = builder.ins().iconst(types::I64, 0);
                jump_block(&mut builder, master_return_block, &[zero]);
            } else {
                jump_block(&mut builder, master_return_block, &[]);
            }
        }

        builder.switch_to_block(master_return_block);
        builder.seal_block(master_return_block);

        let final_res = if has_ret {
            let res = builder.block_params(master_return_block)[0];
            Some(res)
        } else {
            None
        };

        if let Some(res) = final_res {
            builder.ins().return_(&[res]);
        } else {
            builder.ins().return_(&[]);
        }

        let zero_pred_blocks = find_zero_pred_blocks(builder.func);
        if !zero_pred_blocks.is_empty() {
            eprintln!(
                "Backend CFG issue in {}: zero-predecessor blocks {:?}",
                func_ir.name, zero_pred_blocks
            );
            if std::env::var_os("MOLT_DUMP_CLIF_ON_CFG_ERROR").is_some() {
                eprintln!("CLIF {}:\n{}", func_ir.name, builder.func.display());
            }
        }

        let finalize_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            builder.seal_all_blocks();
            builder.finalize();
        }));
        if let Err(payload) = finalize_result {
            eprintln!("Backend panic while finalizing function {}", func_ir.name);
            std::panic::resume_unwind(payload);
        }

        if let Some(config) = should_dump_ir()
            && dump_ir_matches(&config, &func_ir.name)
        {
            dump_ir_ops(&func_ir, &config.mode);
        }

        if let Ok(filter) = std::env::var("MOLT_DUMP_CLIF")
            && (filter == "1" || filter == func_ir.name || func_ir.name.contains(&filter))
        {
            eprintln!("CLIF {}:\n{}", func_ir.name, self.ctx.func.display());
        }

        let id = match self
            .module
            .declare_function(&func_ir.name, Linkage::Export, &self.ctx.func.signature)
        {
            Ok(id) => id,
            Err(e) => {
                let err_str = format!("{e}");
                if err_str.contains("IncompatibleSignature") || err_str.contains("incompatible with previous declaration") {
                    eprintln!(
                        "WARNING: signature mismatch for `{}`; emitting trap stub",
                        func_ir.name
                    );
                    // The function was already forward-declared with a different
                    // signature.  Look up the existing declaration and emit a
                    // trap stub so the linker finds a definition.
                    if let Some(cranelift_module::FuncOrDataId::Func(existing_id)) =
                        self.module.get_name(&func_ir.name)
                    {
                        let existing_sig = self
                            .module
                            .declarations()
                            .get_function_decl(existing_id)
                            .signature
                            .clone();
                        if let Err(stub_err) = Self::emit_trap_stub(
                            &mut self.module,
                            existing_id,
                            &existing_sig,
                            &func_ir.name,
                        ) {
                            eprintln!(
                                "  -> trap stub failed for {}: {}",
                                func_ir.name, stub_err
                            );
                        } else {
                            // Mark as defined so the post-compilation trap
                            // stub loop does not attempt a duplicate stub.
                            self.defined_func_names.insert(func_ir.name.clone());
                        }
                    }
                    self.module.clear_context(&mut self.ctx);
                    return;
                }
                panic!("declare_function failed for {}: {}", func_ir.name, e);
            }
        };
        // When opt_level=none there are no optimization passes that can
        // panic, so we skip the expensive clone + catch_unwind path.  This
        // saves a full IR deep-copy per function (~10-20% of dev compile time
        // for large modules).
        static OPT_LEVEL_NONE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let skip_resilience = *OPT_LEVEL_NONE.get_or_init(|| {
            crate::env_setting("MOLT_BACKEND_OPT_LEVEL")
                .as_deref() == Some("none")
        });
        let func_snapshot = if skip_resilience {
            None
        } else {
            // Clone the function *before* handing it to the optimizer — if an
            // optimization pass panics (e.g. Cranelift remove_constant_phis
            // assertion at cranelift-codegen 0.128) the in-place IR may be
            // partially mutated and unusable.  The clone lets us retry with a
            // lower optimization level on the pristine IR.
            Some(self.ctx.func.clone())
        };
        let define_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.module
                .define_function(id, &mut self.ctx)
                .map_err(Box::new)
        }));
        match define_result {
            Ok(Ok(())) => {
                self.defined_func_names.insert(func_ir.name.clone());
            }
            Ok(Err(err)) => {
                let err_text = format!("{err:?}");
                eprintln!(
                    "Backend verification failed in {}: {err_text}",
                    func_ir.name
                );
                if let Some(config) = should_dump_ir()
                    && dump_ir_matches(&config, &func_ir.name)
                {
                    dump_ir_ops(&func_ir, &config.mode);
                }
                if let Ok(flag) = std::env::var("MOLT_DUMP_CLIF_ON_ERROR") {
                    let clif = self.ctx.func.display().to_string();
                    if let Some(inst) = parse_inst_id(&err_text) {
                        let needle = format!("inst{inst}");
                        let lines: Vec<&str> = clif.lines().collect();
                        let mut hit = None;
                        for (idx, line) in lines.iter().enumerate() {
                            if line.contains(&needle) {
                                hit = Some(idx);
                                break;
                            }
                        }
                        if let Some(center) = hit {
                            let start = center.saturating_sub(3);
                            let end = (center + 3).min(lines.len().saturating_sub(1));
                            eprintln!("CLIF snippet for {} around {}:", func_ir.name, needle);
                            for (offset, line) in lines[start..=end].iter().enumerate() {
                                let idx = start + offset;
                                eprintln!("{:04}: {}", idx + 1, line);
                            }
                        } else if flag == "full" {
                            eprintln!("CLIF {}:\n{}", func_ir.name, clif);
                        }
                    } else if flag == "full" {
                        eprintln!("CLIF {}:\n{}", func_ir.name, clif);
                    }
                }
                panic!("Backend compilation failed");
            }
            Err(payload) => {
                // ── Optimizer panic resilience ──────────────────────────
                // An optimization pass (typically `remove_constant_phis`)
                // hit an internal assertion.  Instead of crashing the
                // entire compilation, retry the function at opt_level=none
                // which skips the problematic pass.  This is the same
                // resilience pattern used by LLVM and GCC when an
                // optimizer pass faults — fall back, warn, keep going.
                let Some(func_snapshot) = func_snapshot else {
                    // skip_resilience was true — should not happen at
                    // opt_level=none, but propagate the panic if it does.
                    std::panic::resume_unwind(payload);
                };
                eprintln!(
                    "WARNING: Cranelift optimizer panic in function `{}`; \
                     retrying at opt_level=none",
                    func_ir.name
                );
                if let Ok(filter) = std::env::var("MOLT_DUMP_CLIF")
                    && (filter == "1" || filter == func_ir.name || func_ir.name.contains(&filter))
                {
                    eprintln!("CLIF (pre-opt) {}:\n{}", func_ir.name, func_snapshot.display());
                }
                // Build a fallback ISA identical to the primary one but
                // with opt_level=none to skip the crashing pass.
                match Self::retry_define_at_opt_none(
                    &mut self.module,
                    id,
                    func_snapshot,
                    &func_ir.name,
                ) {
                    Ok(()) => {
                        self.defined_func_names.insert(func_ir.name.clone());
                        eprintln!(
                            "  -> {} compiled successfully at opt_level=none",
                            func_ir.name
                        );
                    }
                    Err(retry_err) => {
                        eprintln!(
                            "  -> retry also failed for {}: {}",
                            func_ir.name, retry_err
                        );
                        // The retry itself failed — emit a trap stub so
                        // compilation can continue for the remaining
                        // functions.  If this function is actually called
                        // at runtime, the process will abort with a clear
                        // message instead of silently misbehaving.
                        eprintln!(
                            "  -> emitting trap stub for {} (function too large for Cranelift)",
                            func_ir.name
                        );
                        match Self::emit_trap_stub(
                            &mut self.module,
                            id,
                            &self.ctx.func.signature,
                            &func_ir.name,
                        ) {
                            Ok(()) => {
                                // Mark as defined so the post-compilation
                                // trap stub loop does not attempt a duplicate.
                                self.defined_func_names.insert(func_ir.name.clone());
                            }
                            Err(stub_err) => {
                                eprintln!(
                                    "  -> trap stub also failed for {}: {}",
                                    func_ir.name, stub_err
                                );
                                std::panic::resume_unwind(payload);
                            }
                        }
                    }
                }
            }
        }
        self.module.clear_context(&mut self.ctx);
    }
}

#[cfg(all(test, feature = "native-backend"))]
mod tests {
    use super::preanalyze_function_ir;
    use crate::{FunctionIR, OpIR};

    #[test]
    fn preanalysis_fuses_control_flow_state_and_cleanup_metadata() {
        let func = FunctionIR {
            name: "molt_main".to_string(),
            params: vec!["arg".to_string()],
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("msg".to_string()),
                    s_value: Some("hi".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "if".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(42),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "else".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "end_if".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "phi".to_string(),
                    out: Some("joined".to_string()),
                    args: Some(vec!["msg".to_string(), "msg".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "state_yield".to_string(),
                    value: Some(7),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "state_label".to_string(),
                    value: Some(42),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "copy".to_string(),
                    args: Some(vec!["msg".to_string()]),
                    out: Some("out".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("out".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
        };

        let analysis = preanalyze_function_ir(&func);

        assert!(analysis.has_ret);
        assert!(analysis.stateful);
        assert_eq!(analysis.if_to_end_if.get(&1), Some(&4));
        assert_eq!(analysis.if_to_else.get(&1), Some(&3));
        assert_eq!(analysis.else_to_end_if.get(&3), Some(&4));
        assert_eq!(analysis.state_ids, vec![7, 42]);
        assert!(analysis.resume_states.contains(&7));
        assert!(analysis.resume_states.contains(&42));
        assert_eq!(analysis.function_exception_label_id, Some(42));
        assert!(analysis.var_names.contains(&"msg_ptr".to_string()));
        assert!(analysis.var_names.contains(&"msg_len".to_string()));
        assert_eq!(analysis.last_use.get("msg"), Some(&8));
        assert_eq!(analysis.last_use.get("out"), Some(&9));
    }
}
