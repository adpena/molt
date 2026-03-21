use super::*;

#[cfg(feature = "native-backend")]
struct FunctionPreanalysis {
    has_ret: bool,
    stateful: bool,
    has_store: bool,
    var_names: Vec<String>,
    last_use: HashMap<String, usize>,
    if_to_end_if: HashMap<usize, usize>,
    if_to_else: HashMap<usize, usize>,
    else_to_end_if: HashMap<usize, usize>,
    state_ids: Vec<i64>,
    resume_states: HashSet<i64>,
    function_exception_label_id: Option<i64>,
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
    let import_cache_disabled = env_setting("MOLT_BACKEND_DISABLE_IMPORT_CACHE")
        .as_deref()
        .map(parse_truthy_env)
        .unwrap_or(false);
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
    let mut var_names: HashSet<String> = HashSet::new();
    let mut last_use = HashMap::new();
    let mut if_to_end_if = HashMap::new();
    let mut if_to_else = HashMap::new();
    let mut else_to_end_if = HashMap::new();
    let mut if_stack: Vec<(usize, Option<usize>)> = Vec::new();
    let mut state_ids = Vec::new();
    let mut seen_state_ids: HashSet<i64> = HashSet::new();
    let mut resume_states = HashSet::new();
    let mut exception_label_ids = HashSet::new();
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

    let mut var_names: Vec<String> = var_names.into_iter().collect();
    var_names.sort();
    let function_exception_label_id = label_positions
        .into_iter()
        .rev()
        .find_map(|(_, id)| exception_label_ids.contains(&id).then_some(id));

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
    }
}

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    pub(crate) fn compile_func(
        &mut self,
        func_ir: FunctionIR,
        task_kinds: &BTreeMap<String, TrampolineKind>,
        task_closure_sizes: &BTreeMap<String, i64>,
        defined_functions: &HashSet<String>,
        closure_functions: &HashSet<String>,
        emit_traces: bool,
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
        } = preanalyze_function_ir(&func_ir);

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

        let mut vars: HashMap<String, Variable> = HashMap::new();
        let param_name_set: HashSet<&str> = func_ir.params.iter().map(String::as_str).collect();
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
        let mut entry_vars: HashMap<String, Value> = HashMap::new();
        let mut state_blocks = HashMap::new();
        let mut import_refs: BTreeMap<&'static str, FuncRef> = BTreeMap::new();
        let mut reachable_blocks: HashSet<Block> = HashSet::new();
        // Cranelift SSA-variable correctness relies on sealing blocks once all predecessors
        // are known. Our IR uses structured control-flow; for `if` this means then/else
        // each have a single predecessor and can be sealed immediately, and the merge block
        // can be sealed once end_if wiring is complete.
        let mut sealed_blocks: HashSet<Block> = HashSet::new();
        let mut is_block_filled = false;
        let mut if_stack: Vec<IfFrame> = Vec::new();
        let mut loop_stack: Vec<LoopFrame> = Vec::new();
        // Map closure function names to their function object variable names
        let mut local_closure_envs: HashMap<String, String> = HashMap::new();
        let mut loop_depth: i32 = 0;
        let mut block_tracked_obj: HashMap<Block, Vec<String>> = HashMap::new();
        let mut block_tracked_ptr: HashMap<Block, Vec<String>> = HashMap::new();

        let entry_block = builder.create_block();
        let master_return_block = builder.create_block();
        if has_ret {
            builder.append_block_param(master_return_block, types::I64);
        }

        reachable_blocks.insert(entry_block);
        builder.switch_to_block(entry_block);

        let local_dec_ref = import_func_ref(
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
            let self_bits = box_ptr_value(&mut builder, *self_ptr);
            def_var_named(&mut builder, &vars, "self", self_bits);
        }

        let profile_enabled_val = local_profile_enabled.map(|local_profile_enabled| {
            let call = builder.ins().call(local_profile_enabled, &[]);
            builder.inst_results(call)[0]
        });

        builder.seal_block(entry_block);
        sealed_blocks.insert(entry_block);

        for state_id in state_ids {
            state_blocks
                .entry(state_id)
                .or_insert_with(|| builder.create_block());
        }

        // 2. Implementation
        let ops = &func_ir.ops;
        let mut skip_ops: HashSet<usize> = HashSet::new();
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
                    "label" | "state_label" | "else" | "end_if" | "loop_end" => {}
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
                    let val = op.value.unwrap();
                    let boxed = box_int(val);
                    let iconst = builder.ins().iconst(types::I64, boxed);
                    def_var_named(&mut builder, &vars, op.out.unwrap(), iconst);
                }
                "const_bigint" => {
                    let s = op.s_value.as_ref().expect("BigInt string not found");
                    let out_name = op.out.unwrap();
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

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bigint_from_str", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[ptr, len]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "const_bool" => {
                    let val = op.value.unwrap();
                    let boxed = box_bool(val);
                    let iconst = builder.ins().iconst(types::I64, boxed);
                    def_var_named(&mut builder, &vars, op.out.unwrap(), iconst);
                }
                "const_none" => {
                    let iconst = builder.ins().iconst(types::I64, box_none());
                    def_var_named(&mut builder, &vars, op.out.unwrap(), iconst);
                }
                "const_not_implemented" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_not_implemented", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "const_ellipsis" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_ellipsis", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "const_float" => {
                    let val = op.f_value.expect("Float value not found");
                    let boxed = box_float(val);
                    let iconst = builder.ins().iconst(types::I64, boxed);
                    def_var_named(&mut builder, &vars, op.out.unwrap(), iconst);
                }
                "const_str" => {
                    let bytes = op
                        .bytes
                        .as_deref()
                        .unwrap_or_else(|| op.s_value.as_ref().unwrap().as_bytes());
                    let out_name = op.out.unwrap();
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

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64)); // bytes ptr
                    sig.params.push(AbiParam::new(types::I64)); // len
                    sig.params.push(AbiParam::new(types::I64)); // out ptr
                    sig.returns.push(AbiParam::new(types::I32)); // status
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);
                    let callee = self
                        .module
                        .declare_function("molt_string_from_bytes", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[ptr, len, out_ptr]);
                    let boxed = builder
                        .ins()
                        .load(types::I64, MemFlags::trusted(), out_ptr, 0);

                    def_var_named(&mut builder, &vars, out_name, boxed);
                }
                "const_bytes" => {
                    let bytes = op.bytes.as_ref().expect("Bytes not found");
                    let out_name = op.out.unwrap();
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

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64)); // bytes ptr
                    sig.params.push(AbiParam::new(types::I64)); // len
                    sig.params.push(AbiParam::new(types::I64)); // out ptr
                    sig.returns.push(AbiParam::new(types::I32)); // status
                    let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);
                    let callee = self
                        .module
                        .declare_function("molt_bytes_from_bytes", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[ptr, len, out_ptr]);
                    let boxed = builder
                        .ins()
                        .load(types::I64, MemFlags::trusted(), out_ptr, 0);

                    def_var_named(&mut builder, &vars, out_name, boxed);
                }
                "add" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        // Both operands known to be f64 — direct float arithmetic.
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fadd(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f)
                    } else if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_add", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let sum = builder.ins().iadd(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, sum);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_add", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_int_value(&mut builder, sum);
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
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs);
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
                        let flt_res = box_float_value(&mut builder, flt_sum);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "inplace_add" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fadd(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f)
                    } else if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_add", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let sum = builder.ins().iadd(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, sum);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_add", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_int_value(&mut builder, sum);
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
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs);
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
                        let flt_res = box_float_value(&mut builder, flt_sum);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_sum_int" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_int", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_sum_int_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_int_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_sum_int_range" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_int_range", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_sum_int_range_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_int_range_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_sum_int_range_iter" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_int_range_iter", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_sum_int_range_iter_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function(
                            "molt_vec_sum_int_range_iter_trusted",
                            Linkage::Import,
                            &sig,
                        )
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_sum_float" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_float", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_sum_float_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_float_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_sum_float_range" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_float_range", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_sum_float_range_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_float_range_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_sum_float_range_iter" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_sum_float_range_iter", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_sum_float_range_iter_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function(
                            "molt_vec_sum_float_range_iter_trusted",
                            Linkage::Import,
                            &sig,
                        )
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_prod_int" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_prod_int", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_prod_int_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_prod_int_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_prod_int_range" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_prod_int_range", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_prod_int_range_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_prod_int_range_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_min_int" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_min_int", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_min_int_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_min_int_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_min_int_range" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_min_int_range", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_min_int_range_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_min_int_range_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_max_int" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_max_int", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_max_int_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_max_int_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_max_int_range" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_max_int_range", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "vec_max_int_range_trusted" => {
                    let args = op.args.as_ref().unwrap();
                    let seq = var_get(&mut builder, &vars, &args[0]).expect("Seq arg not found");
                    let acc = var_get(&mut builder, &vars, &args[1]).expect("Acc arg not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("Start arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_vec_max_int_range_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "sub" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("LHS not found in {} op {}", func_ir.name, op_idx)
                    });
                    let rhs = var_get(&mut builder, &vars, &args[1]).unwrap_or_else(|| {
                        panic!("RHS not found in {} op {}", func_ir.name, op_idx)
                    });
                    let res = if op.fast_float.unwrap_or(false) {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fsub(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f)
                    } else if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let diff = builder.ins().isub(lhs_val, rhs_val);
                        box_int_value(&mut builder, diff)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_int_value(&mut builder, diff);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_sub", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs);
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
                        let flt_res = box_float_value(&mut builder, flt_diff);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "inplace_sub" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("LHS not found in {} op {}", func_ir.name, op_idx)
                    });
                    let rhs = var_get(&mut builder, &vars, &args[1]).unwrap_or_else(|| {
                        panic!("RHS not found in {} op {}", func_ir.name, op_idx)
                    });
                    let res = if op.fast_float.unwrap_or(false) {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fsub(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f)
                    } else if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let diff = builder.ins().isub(lhs_val, rhs_val);
                        box_int_value(&mut builder, diff)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_int_value(&mut builder, diff);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_sub", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs);
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
                        let flt_res = box_float_value(&mut builder, flt_diff);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "mul" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fmul(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f)
                    } else if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_mul", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let prod = builder.ins().imul(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, prod);
                        let fits_inline = int_value_fits_inline(&mut builder, prod);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_mul", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let prod = builder.ins().imul(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, prod);
                        let fits_inline = int_value_fits_inline(&mut builder, prod);
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
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs);
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
                        let flt_res = box_float_value(&mut builder, flt_prod);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "inplace_mul" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fmul(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f)
                    } else if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_mul", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let prod = builder.ins().imul(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, prod);
                        let fits_inline = int_value_fits_inline(&mut builder, prod);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_mul", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let prod = builder.ins().imul(lhs_val, rhs_val);
                        let fast_res = box_int_value(&mut builder, prod);
                        let fits_inline = int_value_fits_inline(&mut builder, prod);
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
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs);
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
                        let flt_res = box_float_value(&mut builder, flt_prod);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bit_or" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_bit_or", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let raw = builder.ins().bor(lhs_val, rhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, raw);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_bit_or", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_int_value(&mut builder, raw);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "inplace_bit_or" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_bit_or", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let raw = builder.ins().bor(lhs_val, rhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, raw);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_bit_or", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_int_value(&mut builder, raw);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bit_and" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_bit_and", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let raw = builder.ins().band(lhs_val, rhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, raw);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_bit_and", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_int_value(&mut builder, raw);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "inplace_bit_and" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_bit_and", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let raw = builder.ins().band(lhs_val, rhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, raw);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_bit_and", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_int_value(&mut builder, raw);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bit_xor" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_bit_xor", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let raw = builder.ins().bxor(lhs_val, rhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, raw);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_bit_xor", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_int_value(&mut builder, raw);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "inplace_bit_xor" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_bit_xor", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let raw = builder.ins().bxor(lhs_val, rhs_val);
                        let fits_inline = int_value_fits_inline(&mut builder, raw);
                        builder
                            .ins()
                            .brif(fits_inline, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let fast_res = box_int_value(&mut builder, raw);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_inplace_bit_xor", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_int_value(&mut builder, raw);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "lshift" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_lshift", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let range_block = builder.create_block();
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
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
                        let fast_res = box_int_value(&mut builder, shifted);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_lshift", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let int_block = builder.create_block();
                        let range_block = builder.create_block();
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_int_value(&mut builder, shifted);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "rshift" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_rshift", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
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
                        let fast_res = box_int_value(&mut builder, shifted);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_rshift", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let int_block = builder.create_block();
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_int_value(&mut builder, shifted);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "matmul" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_matmul", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "div" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        // Both operands known to be f64 — direct float division.
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let result_f = builder.ins().fdiv(lhs_f, rhs_f);
                        box_float_value(&mut builder, result_f)
                    } else if op.fast_int.unwrap_or(false) {
                        // Both operands known to be int — inline sdiv with
                        // Python floor-division semantics (rounds toward −∞).
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_div", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let one = builder.ins().iconst(types::I64, 1);
                        let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                        builder
                            .ins()
                            .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        // Truncating division + remainder for floor adjustment.
                        let quot = builder.ins().sdiv(lhs_val, rhs_val);
                        let rem = builder.ins().srem(lhs_val, rhs_val);
                        // Python floor-div: if remainder != 0 and operands
                        // have different signs, subtract 1 from the quotient.
                        let rem_nonzero = builder.ins().icmp(IntCC::NotEqual, rem, zero);
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
                        let quot_minus_one = builder.ins().isub(quot, one);
                        let floor_quot = builder.ins().select(adjust, quot_minus_one, quot);
                        let fast_res = box_int_value(&mut builder, floor_quot);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_div", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let one = builder.ins().iconst(types::I64, 1);
                        let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                        let fast_block = builder.create_block();
                        builder
                            .ins()
                            .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        // Inline sdiv with Python floor-division adjustment.
                        let quot = builder.ins().sdiv(lhs_val, rhs_val);
                        let rem = builder.ins().srem(lhs_val, rhs_val);
                        let rem_nonzero = builder.ins().icmp(IntCC::NotEqual, rem, zero);
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
                        let quot_minus_one = builder.ins().isub(quot, one);
                        let floor_quot = builder.ins().select(adjust, quot_minus_one, quot);
                        let fast_res = box_int_value(&mut builder, floor_quot);
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
                        // Inline float fast path: if both operands are floats, do f64 div directly.
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs);
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
                        let flt_res = box_float_value(&mut builder, flt_quot);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "floordiv" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_floordiv", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let zero = builder.ins().iconst(types::I64, 0);
                        let one = builder.ins().iconst(types::I64, 1);
                        let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                        builder
                            .ins()
                            .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                        builder.switch_to_block(fast_block);
                        builder.seal_block(fast_block);
                        let quot = builder.ins().sdiv(lhs_val, rhs_val);
                        let rem = builder.ins().srem(lhs_val, rhs_val);
                        let rem_nonzero = builder.ins().icmp(IntCC::NotEqual, rem, zero);
                        let lhs_neg = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, zero);
                        let rhs_neg = builder.ins().icmp(IntCC::SignedLessThan, rhs_val, zero);
                        let sign_diff = builder.ins().bxor(lhs_neg, rhs_neg);
                        let adjust = builder.ins().band(rem_nonzero, sign_diff);
                        let quot_minus_one = builder.ins().isub(quot, one);
                        let floor_quot = builder.ins().select(adjust, quot_minus_one, quot);
                        let fast_res = box_int_value(&mut builder, floor_quot);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_floordiv", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let quot = builder.ins().sdiv(lhs_val, rhs_val);
                        let rem = builder.ins().srem(lhs_val, rhs_val);
                        let rem_nonzero = builder.ins().icmp(IntCC::NotEqual, rem, zero);
                        let lhs_neg = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, zero);
                        let rhs_neg = builder.ins().icmp(IntCC::SignedLessThan, rhs_val, zero);
                        let sign_diff = builder.ins().bxor(lhs_neg, rhs_neg);
                        let adjust = builder.ins().band(rem_nonzero, sign_diff);
                        let quot_minus_one = builder.ins().isub(quot, one);
                        let floor_quot = builder.ins().select(adjust, quot_minus_one, quot);
                        let fast_res = box_int_value(&mut builder, floor_quot);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "mod" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_mod", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let fast_block = builder.create_block();
                        let slow_block = builder.create_block();
                        builder.set_cold_block(slow_block);
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
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
                        let fast_res = box_int_value(&mut builder, mod_val);
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_mod", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_int_value(&mut builder, mod_val);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "pow" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_pow", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "pow_mod" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let modulus = var_get(&mut builder, &vars, &args[2]).expect("Mod not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_pow_mod", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs, *modulus]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "round" => {
                    let args = op.args.as_ref().unwrap();
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Round arg not found");
                    let ndigits =
                        var_get(&mut builder, &vars, &args[1]).expect("Round ndigits not found");
                    let has_ndigits = var_get(&mut builder, &vars, &args[2])
                        .expect("Round ndigits flag not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_round", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*val, *ndigits, *has_ndigits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "trunc" => {
                    let args = op.args.as_ref().unwrap();
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Trunc arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_trunc", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "len" => {
                    let args = op.args.as_ref().unwrap();
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Len arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_len", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "id" => {
                    let args = op.args.as_ref().unwrap();
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Id arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_id", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "ord" => {
                    let args = op.args.as_ref().unwrap();
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Ord arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_ord", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "chr" => {
                    let args = op.args.as_ref().unwrap();
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Chr arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_chr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "callargs_new" => {
                    let out_name = op.out.unwrap();
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
                    let args = op.args.as_ref().unwrap();
                    let out_name = op.out.unwrap();
                    let size = builder.ins().iconst(types::I64, box_int(args.len() as i64));

                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let new_callee = self
                        .module
                        .declare_function("molt_list_builder_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let builder_ptr = builder.inst_results(new_call)[0];

                    let mut append_sig = self.module.make_signature();
                    append_sig.params.push(AbiParam::new(types::I64));
                    append_sig.params.push(AbiParam::new(types::I64));
                    let append_callee = self
                        .module
                        .declare_function("molt_list_builder_append", Linkage::Import, &append_sig)
                        .unwrap();
                    let append_local = self
                        .module
                        .declare_func_in_func(append_callee, builder.func);
                    for name in args {
                        let val = var_get(&mut builder, &vars, name).unwrap_or_else(|| {
                            panic!("List elem not found in {} op {}", func_ir.name, op_idx)
                        });
                        builder.ins().call(append_local, &[builder_ptr, *val]);
                    }

                    let mut finish_sig = self.module.make_signature();
                    finish_sig.params.push(AbiParam::new(types::I64));
                    finish_sig.returns.push(AbiParam::new(types::I64));
                    let finish_callee = self
                        .module
                        .declare_function("molt_list_builder_finish", Linkage::Import, &finish_sig)
                        .unwrap();
                    let finish_local = self
                        .module
                        .declare_func_in_func(finish_callee, builder.func);
                    let finish_call = builder.ins().call(finish_local, &[builder_ptr]);
                    let list_bits = builder.inst_results(finish_call)[0];
                    def_var_named(&mut builder, &vars, out_name, list_bits);
                }
                "callargs_push_pos" => {
                    let args = op.args.as_ref().unwrap();
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
                    let args = op.args.as_ref().unwrap();
                    let builder_ptr =
                        var_get(&mut builder, &vars, &args[0]).expect("Callargs builder not found");
                    let name =
                        var_get(&mut builder, &vars, &args[1]).expect("Callargs name not found");
                    let val =
                        var_get(&mut builder, &vars, &args[2]).expect("Callargs value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_callargs_push_kw", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder
                        .ins()
                        .call(local_callee, &[*builder_ptr, *name, *val]);
                }
                "callargs_expand_star" => {
                    let args = op.args.as_ref().unwrap();
                    let builder_ptr =
                        var_get(&mut builder, &vars, &args[0]).expect("Callargs builder not found");
                    let iterable = var_get(&mut builder, &vars, &args[1])
                        .expect("Callargs iterable not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_callargs_expand_star", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*builder_ptr, *iterable]);
                }
                "callargs_expand_kwstar" => {
                    let args = op.args.as_ref().unwrap();
                    let builder_ptr =
                        var_get(&mut builder, &vars, &args[0]).expect("Callargs builder not found");
                    let mapping =
                        var_get(&mut builder, &vars, &args[1]).expect("Callargs mapping not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_callargs_expand_kwstar", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*builder_ptr, *mapping]);
                }
                "range_new" => {
                    let args = op.args.as_ref().unwrap();
                    let start =
                        var_get(&mut builder, &vars, &args[0]).expect("Range start not found");
                    let stop =
                        var_get(&mut builder, &vars, &args[1]).expect("Range stop not found");
                    let step =
                        var_get(&mut builder, &vars, &args[2]).expect("Range step not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_range_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*start, *stop, *step]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "list_from_range" => {
                    let args = op.args.as_ref().unwrap();
                    let start = var_get(&mut builder, &vars, &args[0])
                        .expect("List-from-range start not found");
                    let stop = var_get(&mut builder, &vars, &args[1])
                        .expect("List-from-range stop not found");
                    let step = var_get(&mut builder, &vars, &args[2])
                        .expect("List-from-range step not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_from_range", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*start, *stop, *step]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "tuple_new" => {
                    let args = op.args.as_ref().unwrap();
                    let out_name = op.out.unwrap();
                    let size = builder.ins().iconst(types::I64, box_int(args.len() as i64));

                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let new_callee = self
                        .module
                        .declare_function("molt_list_builder_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let builder_ptr = builder.inst_results(new_call)[0];

                    let mut append_sig = self.module.make_signature();
                    append_sig.params.push(AbiParam::new(types::I64));
                    append_sig.params.push(AbiParam::new(types::I64));
                    let append_callee = self
                        .module
                        .declare_function("molt_list_builder_append", Linkage::Import, &append_sig)
                        .unwrap();
                    let append_local = self
                        .module
                        .declare_func_in_func(append_callee, builder.func);
                    for name in args {
                        let val = var_get(&mut builder, &vars, name).expect("Tuple elem not found");
                        builder.ins().call(append_local, &[builder_ptr, *val]);
                    }

                    let mut finish_sig = self.module.make_signature();
                    finish_sig.params.push(AbiParam::new(types::I64));
                    finish_sig.returns.push(AbiParam::new(types::I64));
                    let finish_callee = self
                        .module
                        .declare_function("molt_tuple_builder_finish", Linkage::Import, &finish_sig)
                        .unwrap();
                    let finish_local = self
                        .module
                        .declare_func_in_func(finish_callee, builder.func);
                    let finish_call = builder.ins().call(finish_local, &[builder_ptr]);
                    let tuple_bits = builder.inst_results(finish_call)[0];
                    def_var_named(&mut builder, &vars, out_name, tuple_bits);
                }
                "list_append" => {
                    let args = op.args.as_ref().unwrap();
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let val = var_get(&mut builder, &vars, &args[1])
                        .expect("List append value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_append", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "list_pop" => {
                    let args = op.args.as_ref().unwrap();
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let idx =
                        var_get(&mut builder, &vars, &args[1]).expect("List pop index not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_pop", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *idx]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "list_extend" => {
                    let args = op.args.as_ref().unwrap();
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let other = var_get(&mut builder, &vars, &args[1])
                        .expect("List extend iterable not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_extend", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *other]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "list_insert" => {
                    let args = op.args.as_ref().unwrap();
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let idx = var_get(&mut builder, &vars, &args[1])
                        .expect("List insert index not found");
                    let val = var_get(&mut builder, &vars, &args[2])
                        .expect("List insert value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_insert", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *idx, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "list_remove" => {
                    let args = op.args.as_ref().unwrap();
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let val = var_get(&mut builder, &vars, &args[1])
                        .expect("List remove value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_remove", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "list_clear" => {
                    let args = op.args.as_ref().unwrap();
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_clear", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "list_copy" => {
                    let args = op.args.as_ref().unwrap();
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_copy", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "list_reverse" => {
                    let args = op.args.as_ref().unwrap();
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_reverse", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "list_count" => {
                    let args = op.args.as_ref().unwrap();
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let val =
                        var_get(&mut builder, &vars, &args[1]).expect("List count value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_count", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "list_index" => {
                    let args = op.args.as_ref().unwrap();
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let val =
                        var_get(&mut builder, &vars, &args[1]).expect("List index value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_index", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "list_index_range" => {
                    let args = op.args.as_ref().unwrap();
                    let list = var_get(&mut builder, &vars, &args[0]).expect("List not found");
                    let val =
                        var_get(&mut builder, &vars, &args[1]).expect("List index value not found");
                    let start =
                        var_get(&mut builder, &vars, &args[2]).expect("List index start not found");
                    let stop =
                        var_get(&mut builder, &vars, &args[3]).expect("List index stop not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_list_index_range", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*list, *val, *start, *stop]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "tuple_from_list" => {
                    let args = op.args.as_ref().unwrap();
                    let list =
                        var_get(&mut builder, &vars, &args[0]).expect("Tuple source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_tuple_from_list", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*list]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_new" => {
                    let args = op.args.as_ref().unwrap();
                    let out_name = op.out.unwrap();
                    let size = builder.ins().iconst(types::I64, (args.len() / 2) as i64);

                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let new_callee = self
                        .module
                        .declare_function("molt_dict_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let dict_bits = builder.inst_results(new_call)[0];

                    let mut set_sig = self.module.make_signature();
                    set_sig.params.push(AbiParam::new(types::I64));
                    set_sig.params.push(AbiParam::new(types::I64));
                    set_sig.params.push(AbiParam::new(types::I64));
                    set_sig.returns.push(AbiParam::new(types::I64));
                    let set_callee = self
                        .module
                        .declare_function("molt_dict_set", Linkage::Import, &set_sig)
                        .unwrap();
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
                    let args = op.args.as_ref().unwrap();
                    let obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Dict source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "set_new" => {
                    let args = op.args.as_ref().unwrap();
                    let out_name = op.out.unwrap();
                    let size = builder.ins().iconst(types::I64, args.len() as i64);

                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let new_callee = self
                        .module
                        .declare_function("molt_set_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let set_bits = builder.inst_results(new_call)[0];

                    if !args.is_empty() {
                        let mut add_sig = self.module.make_signature();
                        add_sig.params.push(AbiParam::new(types::I64));
                        add_sig.params.push(AbiParam::new(types::I64));
                        add_sig.returns.push(AbiParam::new(types::I64));
                        let add_callee = self
                            .module
                            .declare_function("molt_set_add", Linkage::Import, &add_sig)
                            .unwrap();
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
                    let args = op.args.as_ref().unwrap();
                    let out_name = op.out.unwrap();
                    let size = builder.ins().iconst(types::I64, args.len() as i64);

                    let mut new_sig = self.module.make_signature();
                    new_sig.params.push(AbiParam::new(types::I64));
                    new_sig.returns.push(AbiParam::new(types::I64));
                    let new_callee = self
                        .module
                        .declare_function("molt_frozenset_new", Linkage::Import, &new_sig)
                        .unwrap();
                    let new_local = self.module.declare_func_in_func(new_callee, builder.func);
                    let new_call = builder.ins().call(new_local, &[size]);
                    let set_bits = builder.inst_results(new_call)[0];

                    if !args.is_empty() {
                        let mut add_sig = self.module.make_signature();
                        add_sig.params.push(AbiParam::new(types::I64));
                        add_sig.params.push(AbiParam::new(types::I64));
                        add_sig.returns.push(AbiParam::new(types::I64));
                        let add_callee = self
                            .module
                            .declare_function("molt_frozenset_add", Linkage::Import, &add_sig)
                            .unwrap();
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
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let key = var_get(&mut builder, &vars, &args[1]).expect("Dict key not found");
                    let default =
                        var_get(&mut builder, &vars, &args[2]).expect("Dict default not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_get", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *key, *default]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_inc" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let key = var_get(&mut builder, &vars, &args[1]).expect("Dict key not found");
                    let delta = var_get(&mut builder, &vars, &args[2])
                        .expect("Dict increment value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_inc", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *key, *delta]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_str_int_inc" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let key = var_get(&mut builder, &vars, &args[1]).expect("Dict key not found");
                    let delta = var_get(&mut builder, &vars, &args[2])
                        .expect("Dict increment value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_str_int_inc", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *key, *delta]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_split_ws_dict_inc" => {
                    let args = op.args.as_ref().unwrap();
                    let line = var_get(&mut builder, &vars, &args[0]).expect("Line not found");
                    let dict = var_get(&mut builder, &vars, &args[1]).expect("Dict not found");
                    let delta = var_get(&mut builder, &vars, &args[2]).expect("Delta not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_split_ws_dict_inc", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*line, *dict, *delta]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "taq_ingest_line" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let line = var_get(&mut builder, &vars, &args[1]).expect("Line not found");
                    let bucket_size =
                        var_get(&mut builder, &vars, &args[2]).expect("Bucket size not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_taq_ingest_line", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*dict, *line, *bucket_size]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_split_sep_dict_inc" => {
                    let args = op.args.as_ref().unwrap();
                    let line = var_get(&mut builder, &vars, &args[0]).expect("Line not found");
                    let sep = var_get(&mut builder, &vars, &args[1]).expect("Separator not found");
                    let dict = var_get(&mut builder, &vars, &args[2]).expect("Dict not found");
                    let delta = var_get(&mut builder, &vars, &args[3]).expect("Delta not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_split_sep_dict_inc", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*line, *sep, *dict, *delta]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_pop" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let key = var_get(&mut builder, &vars, &args[1]).expect("Dict key not found");
                    let default =
                        var_get(&mut builder, &vars, &args[2]).expect("Dict default not found");
                    let has_default = var_get(&mut builder, &vars, &args[3])
                        .expect("Dict default flag not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_pop", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*dict, *key, *default, *has_default]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_setdefault" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let key = var_get(&mut builder, &vars, &args[1]).expect("Dict key not found");
                    let default =
                        var_get(&mut builder, &vars, &args[2]).expect("Dict default not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_setdefault", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *key, *default]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_setdefault_empty_list" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let key = var_get(&mut builder, &vars, &args[1]).expect("Dict key not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_setdefault_empty_list", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *key]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_update" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let other = var_get(&mut builder, &vars, &args[1])
                        .expect("Dict update iterable not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_update", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *other]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_clear" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_clear", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_copy" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_copy", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_popitem" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_popitem", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_update_kwstar" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let other = var_get(&mut builder, &vars, &args[1])
                        .expect("Dict update mapping not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_update_kwstar", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict, *other]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "set_add" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let key_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Set key not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_add", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *key_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "frozenset_add" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Frozenset not found");
                    let key_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Frozenset key not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_frozenset_add", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *key_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "set_discard" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let key_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Set key not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_discard", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *key_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "set_remove" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let key_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Set key not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_remove", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *key_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "set_pop" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_pop", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "set_update" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let other_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Set update arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_update", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *other_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "set_intersection_update" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let other_bits = var_get(&mut builder, &vars, &args[1])
                        .expect("Set intersection update arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_intersection_update", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *other_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "set_difference_update" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let other_bits = var_get(&mut builder, &vars, &args[1])
                        .expect("Set difference update arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_difference_update", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *other_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "set_symdiff_update" => {
                    let args = op.args.as_ref().unwrap();
                    let set_bits = var_get(&mut builder, &vars, &args[0]).expect("Set not found");
                    let other_bits = var_get(&mut builder, &vars, &args[1])
                        .expect("Set symdiff update arg not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_symdiff_update", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*set_bits, *other_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_keys" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_keys", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_values" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_values", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_items" => {
                    let args = op.args.as_ref().unwrap();
                    let dict = var_get(&mut builder, &vars, &args[0]).expect("Dict not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_items", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*dict]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "tuple_count" => {
                    let args = op.args.as_ref().unwrap();
                    let tuple = var_get(&mut builder, &vars, &args[0]).expect("Tuple not found");
                    let val = var_get(&mut builder, &vars, &args[1])
                        .expect("Tuple count value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_tuple_count", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*tuple, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "tuple_index" => {
                    let args = op.args.as_ref().unwrap();
                    let tuple = var_get(&mut builder, &vars, &args[0]).expect("Tuple not found");
                    let val = var_get(&mut builder, &vars, &args[1])
                        .expect("Tuple index value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_tuple_index", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*tuple, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "iter" => {
                    let args = op.args.as_ref().unwrap();
                    let obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Iter source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_iter_checked", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "enumerate" => {
                    let args = op.args.as_ref().unwrap();
                    let iterable = var_get(&mut builder, &vars, &args[0])
                        .expect("Enumerate iterable not found");
                    let start =
                        var_get(&mut builder, &vars, &args[1]).expect("Enumerate start not found");
                    let has_start = var_get(&mut builder, &vars, &args[2])
                        .expect("Enumerate has_start not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_enumerate", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*iterable, *start, *has_start]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "aiter" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0])
                        .expect("Async iter source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_aiter", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "iter_next" => {
                    let args = op.args.as_ref().unwrap();
                    let iter = var_get(&mut builder, &vars, &args[0]).expect("Iter not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_iter_next", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*iter]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "anext" => {
                    let args = op.args.as_ref().unwrap();
                    let iter =
                        var_get(&mut builder, &vars, &args[0]).expect("Async iter not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_anext", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*iter]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "asyncgen_new" => {
                    let args = op.args.as_ref().unwrap();
                    let gen_obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Generator not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_asyncgen_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*gen_obj]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "asyncgen_shutdown" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_asyncgen_shutdown", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "gen_send" => {
                    let args = op.args.as_ref().unwrap();
                    let gen_obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Generator not found");
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Send value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_generator_send", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*gen_obj, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "gen_throw" => {
                    let args = op.args.as_ref().unwrap();
                    let gen_obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Generator not found");
                    let val =
                        var_get(&mut builder, &vars, &args[1]).expect("Throw value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_generator_throw", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*gen_obj, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "gen_close" => {
                    let args = op.args.as_ref().unwrap();
                    let gen_obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Generator not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_generator_close", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*gen_obj]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "is_generator" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Obj not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is_generator", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "is_bound_method" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Obj not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is_bound_method", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "is_callable" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Obj not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is_callable", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "index" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Obj not found");
                    let idx = var_get(&mut builder, &vars, &args[1]).expect("Index not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_index", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "store_index" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Obj not found in {} op {}", func_ir.name, op_idx)
                    });
                    let idx = var_get(&mut builder, &vars, &args[1]).unwrap_or_else(|| {
                        panic!("Index not found in {} op {}", func_ir.name, op_idx)
                    });
                    let val = var_get(&mut builder, &vars, &args[2]).unwrap_or_else(|| {
                        panic!("Value not found in {} op {}", func_ir.name, op_idx)
                    });
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_store_index", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_set" => {
                    let args = op.args.as_ref().unwrap();
                    let dict_bits = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Dict not found in {} op {}", func_ir.name, op_idx)
                    });
                    let key_bits = var_get(&mut builder, &vars, &args[1]).unwrap_or_else(|| {
                        panic!("Key not found in {} op {}", func_ir.name, op_idx)
                    });
                    let val_bits = var_get(&mut builder, &vars, &args[2]).unwrap_or_else(|| {
                        panic!("Value not found in {} op {}", func_ir.name, op_idx)
                    });
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_set", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*dict_bits, *key_bits, *val_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dict_update_missing" => {
                    let args = op.args.as_ref().unwrap();
                    let dict_bits = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Dict not found in {} op {}", func_ir.name, op_idx)
                    });
                    let key_bits = var_get(&mut builder, &vars, &args[1]).unwrap_or_else(|| {
                        panic!("Key not found in {} op {}", func_ir.name, op_idx)
                    });
                    let val_bits = var_get(&mut builder, &vars, &args[2]).unwrap_or_else(|| {
                        panic!("Value not found in {} op {}", func_ir.name, op_idx)
                    });
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dict_update_missing", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*dict_bits, *key_bits, *val_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "del_index" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Obj not found in {} op {}", func_ir.name, op_idx)
                    });
                    let idx = var_get(&mut builder, &vars, &args[1]).unwrap_or_else(|| {
                        panic!("Index not found in {} op {}", func_ir.name, op_idx)
                    });
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_del_index", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "slice" => {
                    let args = op.args.as_ref().unwrap();
                    let target =
                        var_get(&mut builder, &vars, &args[0]).expect("Slice target not found");
                    let start =
                        var_get(&mut builder, &vars, &args[1]).expect("Slice start not found");
                    let end = var_get(&mut builder, &vars, &args[2]).expect("Slice end not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*target, *start, *end]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "slice_new" => {
                    let args = op.args.as_ref().unwrap();
                    let start =
                        var_get(&mut builder, &vars, &args[0]).expect("Slice start not found");
                    let stop =
                        var_get(&mut builder, &vars, &args[1]).expect("Slice stop not found");
                    let step =
                        var_get(&mut builder, &vars, &args[2]).expect("Slice step not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_slice_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*start, *stop, *step]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytes_find" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Find haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Find needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_find", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytes_find_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_find_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytearray_find" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Find haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Find needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_find", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytearray_find_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_find_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_find" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Find haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Find needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_find", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_find_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_find_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_format" => {
                    let args = op.args.as_ref().unwrap();
                    let val =
                        var_get(&mut builder, &vars, &args[0]).expect("Format value not found");
                    let spec =
                        var_get(&mut builder, &vars, &args[1]).expect("Format spec not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_format_builtin", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val, *spec]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_startswith" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Startswith haystack not found");
                    let needle = var_get(&mut builder, &vars, &args[1])
                        .expect("Startswith needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_startswith", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_startswith_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_startswith_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytes_startswith" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Startswith haystack not found");
                    let needle = var_get(&mut builder, &vars, &args[1])
                        .expect("Startswith needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_startswith", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytes_startswith_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_startswith_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytearray_startswith" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Startswith haystack not found");
                    let needle = var_get(&mut builder, &vars, &args[1])
                        .expect("Startswith needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_startswith", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytearray_startswith_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_startswith_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_endswith" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Endswith haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Endswith needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_endswith", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_endswith_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_endswith_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytes_endswith" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Endswith haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Endswith needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_endswith", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytes_endswith_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_endswith_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytearray_endswith" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Endswith haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Endswith needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_endswith", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytearray_endswith_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    for _ in 0..6 {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_endswith_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_count" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Count haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Count needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_count", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytes_count" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Count haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Count needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_count", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytearray_count" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Count haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Count needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_count", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_count_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_count_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytes_count_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_count_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytearray_count_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_count_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(
                        local_callee,
                        &[*hay, *needle, *start, *end, *has_start, *has_end],
                    );
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "env_get" => {
                    let args = op.args.as_ref().unwrap();
                    let key = var_get(&mut builder, &vars, &args[0]).expect("Env key not found");
                    let default =
                        var_get(&mut builder, &vars, &args[1]).expect("Env default not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_env_get", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*key, *default]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_join" => {
                    let args = op.args.as_ref().unwrap();
                    let sep =
                        var_get(&mut builder, &vars, &args[0]).expect("Join separator not found");
                    let items =
                        var_get(&mut builder, &vars, &args[1]).expect("Join items not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_join", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*sep, *items]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_split" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Split haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Split needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_split", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_split_max" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Split haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Split needle not found");
                    let maxsplit =
                        var_get(&mut builder, &vars, &args[2]).expect("Split maxsplit not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_split_max", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *maxsplit]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "statistics_mean_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_statistics_mean_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*seq, *start, *end, *has_start, *has_end]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "statistics_stdev_slice" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_statistics_stdev_slice", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*seq, *start, *end, *has_start, *has_end]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_lower" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Lower string not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_lower", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_upper" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Upper string not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_upper", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_capitalize" => {
                    let args = op.args.as_ref().unwrap();
                    let hay = var_get(&mut builder, &vars, &args[0])
                        .expect("Capitalize string not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_capitalize", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_strip" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Strip string not found");
                    let chars =
                        var_get(&mut builder, &vars, &args[1]).expect("Strip chars not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_strip", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *chars]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_lstrip" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Lstrip string not found");
                    let chars =
                        var_get(&mut builder, &vars, &args[1]).expect("Lstrip chars not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_lstrip", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *chars]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_rstrip" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Rstrip string not found");
                    let chars =
                        var_get(&mut builder, &vars, &args[1]).expect("Rstrip chars not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_rstrip", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *chars]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_replace" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Replace haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Replace needle not found");
                    let replacement = var_get(&mut builder, &vars, &args[2])
                        .expect("Replace replacement not found");
                    let count =
                        var_get(&mut builder, &vars, &args[3]).expect("Replace count not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_replace", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *replacement, *count]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytes_split" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Split haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Split needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_split", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytes_split_max" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Split haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Split needle not found");
                    let maxsplit =
                        var_get(&mut builder, &vars, &args[2]).expect("Split maxsplit not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_split_max", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *maxsplit]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytearray_split" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Split haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Split needle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_split", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*hay, *needle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytearray_split_max" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Split haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Split needle not found");
                    let maxsplit =
                        var_get(&mut builder, &vars, &args[2]).expect("Split maxsplit not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_split_max", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *maxsplit]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytes_replace" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Replace haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Replace needle not found");
                    let replacement = var_get(&mut builder, &vars, &args[2])
                        .expect("Replace replacement not found");
                    let count =
                        var_get(&mut builder, &vars, &args[3]).expect("Replace count not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_replace", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *replacement, *count]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytearray_replace" => {
                    let args = op.args.as_ref().unwrap();
                    let hay =
                        var_get(&mut builder, &vars, &args[0]).expect("Replace haystack not found");
                    let needle =
                        var_get(&mut builder, &vars, &args[1]).expect("Replace needle not found");
                    let replacement = var_get(&mut builder, &vars, &args[2])
                        .expect("Replace replacement not found");
                    let count =
                        var_get(&mut builder, &vars, &args[3]).expect("Replace count not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_replace", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*hay, *needle, *replacement, *count]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytes_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Bytes source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytes_from_str" => {
                    let args = op.args.as_ref().unwrap();
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Bytes source not found");
                    let encoding =
                        var_get(&mut builder, &vars, &args[1]).expect("Bytes encoding not found");
                    let errors =
                        var_get(&mut builder, &vars, &args[2]).expect("Bytes errors not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytes_from_str", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*src, *encoding, *errors]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytearray_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Bytearray source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bytearray_from_str" => {
                    let args = op.args.as_ref().unwrap();
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Bytearray source not found");
                    let encoding = var_get(&mut builder, &vars, &args[1])
                        .expect("Bytearray encoding not found");
                    let errors =
                        var_get(&mut builder, &vars, &args[2]).expect("Bytearray errors not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bytearray_from_str", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*src, *encoding, *errors]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "float_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Float source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_float_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "int_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Int value not found");
                    let base = var_get(&mut builder, &vars, &args[1]).expect("Int base not found");
                    let has_base =
                        var_get(&mut builder, &vars, &args[2]).expect("Int base flag not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_int_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val, *base, *has_base]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "complex_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let val =
                        var_get(&mut builder, &vars, &args[0]).expect("Complex value not found");
                    let imag =
                        var_get(&mut builder, &vars, &args[1]).expect("Complex imag not found");
                    let has_imag =
                        var_get(&mut builder, &vars, &args[2]).expect("Complex flag not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_complex_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val, *imag, *has_imag]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "intarray_from_seq" => {
                    let args = op.args.as_ref().unwrap();
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Intarray source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_intarray_from_seq", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "memoryview_new" => {
                    let args = op.args.as_ref().unwrap();
                    let src = var_get(&mut builder, &vars, &args[0])
                        .expect("Memoryview source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_memoryview_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "memoryview_tobytes" => {
                    let args = op.args.as_ref().unwrap();
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Memoryview value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_memoryview_tobytes", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "memoryview_cast" => {
                    let args = op.args.as_ref().unwrap();
                    let view =
                        var_get(&mut builder, &vars, &args[0]).expect("Memoryview not found");
                    let format = var_get(&mut builder, &vars, &args[1])
                        .expect("Memoryview format not found");
                    let shape =
                        var_get(&mut builder, &vars, &args[2]).expect("Memoryview shape not found");
                    let has_shape = var_get(&mut builder, &vars, &args[3])
                        .expect("Memoryview shape flag not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_memoryview_cast", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*view, *format, *shape, *has_shape]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "buffer2d_new" => {
                    let args = op.args.as_ref().unwrap();
                    let rows =
                        var_get(&mut builder, &vars, &args[0]).expect("Buffer2D rows not found");
                    let cols =
                        var_get(&mut builder, &vars, &args[1]).expect("Buffer2D cols not found");
                    let init =
                        var_get(&mut builder, &vars, &args[2]).expect("Buffer2D init not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_buffer2d_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*rows, *cols, *init]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "buffer2d_get" => {
                    let args = op.args.as_ref().unwrap();
                    let buf = var_get(&mut builder, &vars, &args[0]).expect("Buffer2D not found");
                    let row =
                        var_get(&mut builder, &vars, &args[1]).expect("Buffer2D row not found");
                    let col =
                        var_get(&mut builder, &vars, &args[2]).expect("Buffer2D col not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_buffer2d_get", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*buf, *row, *col]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "buffer2d_set" => {
                    let args = op.args.as_ref().unwrap();
                    let buf = var_get(&mut builder, &vars, &args[0]).expect("Buffer2D not found");
                    let row =
                        var_get(&mut builder, &vars, &args[1]).expect("Buffer2D row not found");
                    let col =
                        var_get(&mut builder, &vars, &args[2]).expect("Buffer2D col not found");
                    let val =
                        var_get(&mut builder, &vars, &args[3]).expect("Buffer2D val not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_buffer2d_set", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*buf, *row, *col, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "buffer2d_matmul" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs =
                        var_get(&mut builder, &vars, &args[0]).expect("Buffer2D lhs not found");
                    let rhs =
                        var_get(&mut builder, &vars, &args[1]).expect("Buffer2D rhs not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_buffer2d_matmul", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "str_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let src = var_get(&mut builder, &vars, &args[0]).expect("Str source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_str_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "repr_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Repr source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_repr_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "ascii_from_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let src =
                        var_get(&mut builder, &vars, &args[0]).expect("Ascii source not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_ascii_from_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*src]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dataclass_new" => {
                    let args = op.args.as_ref().unwrap();
                    let name =
                        var_get(&mut builder, &vars, &args[0]).expect("Dataclass name not found");
                    let fields =
                        var_get(&mut builder, &vars, &args[1]).expect("Dataclass fields not found");
                    let values =
                        var_get(&mut builder, &vars, &args[2]).expect("Dataclass values not found");
                    let flags =
                        var_get(&mut builder, &vars, &args[3]).expect("Dataclass flags not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dataclass_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*name, *fields, *values, *flags]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dataclass_get" => {
                    let args = op.args.as_ref().unwrap();
                    let obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Dataclass object not found");
                    let idx =
                        var_get(&mut builder, &vars, &args[1]).expect("Dataclass index not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dataclass_get", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dataclass_set" => {
                    let args = op.args.as_ref().unwrap();
                    let obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Dataclass object not found");
                    let idx =
                        var_get(&mut builder, &vars, &args[1]).expect("Dataclass index not found");
                    let val =
                        var_get(&mut builder, &vars, &args[2]).expect("Dataclass value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dataclass_set", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "dataclass_set_class" => {
                    let args = op.args.as_ref().unwrap();
                    let obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Dataclass object not found");
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_dataclass_set_class", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "lt" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let cmp = builder.ins().fcmp(FloatCC::LessThan, lhs_f, rhs_f);
                        box_bool_value(&mut builder, cmp)
                    } else if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let cmp = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_bool_value(&mut builder, cmp);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_lt", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs);
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
                        let flt_res = box_bool_value(&mut builder, fcmp);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "le" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let cmp = builder.ins().fcmp(FloatCC::LessThanOrEqual, lhs_f, rhs_f);
                        box_bool_value(&mut builder, cmp)
                    } else if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let cmp =
                            builder
                                .ins()
                                .icmp(IntCC::SignedLessThanOrEqual, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_bool_value(&mut builder, cmp);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_le", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs);
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
                        let flt_res = box_bool_value(&mut builder, fcmp);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "gt" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let cmp = builder.ins().fcmp(FloatCC::GreaterThan, lhs_f, rhs_f);
                        box_bool_value(&mut builder, cmp)
                    } else if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let cmp = builder
                            .ins()
                            .icmp(IntCC::SignedGreaterThan, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_bool_value(&mut builder, cmp);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_gt", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs);
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
                        let flt_res = box_bool_value(&mut builder, fcmp);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "ge" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_float.unwrap_or(false) {
                        let lhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *lhs);
                        let rhs_f = builder.ins().bitcast(types::F64, MemFlags::new(), *rhs);
                        let cmp = builder
                            .ins()
                            .fcmp(FloatCC::GreaterThanOrEqual, lhs_f, rhs_f);
                        box_bool_value(&mut builder, cmp)
                    } else if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let cmp =
                            builder
                                .ins()
                                .icmp(IntCC::SignedGreaterThanOrEqual, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_bool_value(&mut builder, cmp);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_ge", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let both_flt = both_float_check(&mut builder, *lhs, *rhs);
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
                        let flt_res = box_bool_value(&mut builder, fcmp);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "eq" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let cmp = builder.ins().icmp(IntCC::Equal, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_bool_value(&mut builder, cmp);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_eq", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "ne" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let res = if op.fast_int.unwrap_or(false) {
                        let lhs_val = unbox_int(&mut builder, *lhs);
                        let rhs_val = unbox_int(&mut builder, *rhs);
                        let cmp = builder.ins().icmp(IntCC::NotEqual, lhs_val, rhs_val);
                        box_bool_value(&mut builder, cmp)
                    } else {
                        let (lhs_xored, lhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *lhs);
                        let (rhs_xored, rhs_val) =
                            fused_tag_check_and_unbox_int(&mut builder, *rhs);
                        let both_int = fused_both_int_check(&mut builder, lhs_xored, rhs_xored);
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
                        let fast_res = box_bool_value(&mut builder, cmp);
                        jump_block(&mut builder, merge_block, &[fast_res]);

                        builder.switch_to_block(slow_block);
                        builder.seal_block(slow_block);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_ne", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                        let slow_res = builder.inst_results(call)[0];
                        jump_block(&mut builder, merge_block, &[slow_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    };
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "string_eq" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_string_eq", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "is" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "not" => {
                    let args = op.args.as_ref().unwrap();
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_not", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "abs" => {
                    let args = op.args.as_ref().unwrap();
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_abs_builtin", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "invert" => {
                    let args = op.args.as_ref().unwrap();
                    let val = var_get(&mut builder, &vars, &args[0]).expect("Value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_invert", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "and" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let truthy = self
                        .module
                        .declare_function("molt_is_truthy", Linkage::Import, &sig)
                        .unwrap();
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
                    emit_inc_ref_obj(&mut builder, res, local_inc_ref_obj);
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "or" => {
                    let args = op.args.as_ref().unwrap();
                    let lhs = var_get(&mut builder, &vars, &args[0]).expect("LHS not found");
                    let rhs = var_get(&mut builder, &vars, &args[1]).expect("RHS not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let truthy = self
                        .module
                        .declare_function("molt_is_truthy", Linkage::Import, &sig)
                        .unwrap();
                    let truthy_ref = self.module.declare_func_in_func(truthy, builder.func);
                    let lhs_call = builder.ins().call(truthy_ref, &[*lhs]);
                    let lhs_val = builder.inst_results(lhs_call)[0];
                    let cond = builder.ins().icmp_imm(IntCC::NotEqual, lhs_val, 0);
                    let res = builder.ins().select(cond, *lhs, *rhs);
                    // Same aliasing hazard as `and` — see comment above.
                    emit_inc_ref_obj(&mut builder, res, local_inc_ref_obj);
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "contains" => {
                    let args = op.args.as_ref().unwrap();
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "print" => {
                    let args = op.args.as_ref().unwrap();
                    let val = if let Some(val) = var_get(&mut builder, &vars, &args[0]) {
                        *val
                    } else {
                        builder.ins().iconst(types::I64, box_none())
                    };

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_print_obj", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[val]);
                }
                "print_newline" => {
                    let sig = self.module.make_signature();
                    let callee = self
                        .module
                        .declare_function("molt_print_newline", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[]);
                }
                "json_parse" => {
                    let args = op.args.as_ref().unwrap();
                    let arg_name = &args[0];
                    if let Some(len) = var_get(&mut builder, &vars, &format!("{}_len", arg_name)) {
                        let ptr = var_get(&mut builder, &vars, &format!("{}_ptr", arg_name))
                            .or_else(|| var_get(&mut builder, &vars, arg_name))
                            .expect("String ptr not found");

                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64)); // ptr
                        sig.params.push(AbiParam::new(types::I64)); // len
                        sig.params.push(AbiParam::new(types::I64)); // out ptr
                        sig.returns.push(AbiParam::new(types::I32)); // status

                        let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);

                        let callee = self
                            .module
                            .declare_function("molt_json_parse_scalar", Linkage::Import, &sig)
                            .unwrap();
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
                        let mut err_sig = self.module.make_signature();
                        err_sig.params.push(AbiParam::new(types::I64));
                        err_sig.returns.push(AbiParam::new(types::I64));
                        let err_callee = self
                            .module
                            .declare_function(
                                "molt_json_parse_scalar_obj",
                                Linkage::Import,
                                &err_sig,
                            )
                            .unwrap();
                        let err_local = self.module.declare_func_in_func(err_callee, builder.func);
                        let err_call = builder.ins().call(err_local, &[*arg_bits]);
                        let err_res = builder.inst_results(err_call)[0];
                        jump_block(&mut builder, merge_block, &[err_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        let res = builder.block_params(merge_block)[0];
                        def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                    } else {
                        let arg_bits =
                            var_get(&mut builder, &vars, arg_name).expect("String arg not found");
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_json_parse_scalar_obj", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*arg_bits]);
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                    }
                }
                "msgpack_parse" => {
                    let args = op.args.as_ref().unwrap();
                    let arg_name = &args[0];
                    if let Some(len) = var_get(&mut builder, &vars, &format!("{}_len", arg_name)) {
                        let ptr = var_get(&mut builder, &vars, &format!("{}_ptr", arg_name))
                            .or_else(|| var_get(&mut builder, &vars, arg_name))
                            .expect("Bytes ptr not found");

                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64)); // ptr
                        sig.params.push(AbiParam::new(types::I64)); // len
                        sig.params.push(AbiParam::new(types::I64)); // out ptr
                        sig.returns.push(AbiParam::new(types::I32)); // status

                        let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);

                        let callee = self
                            .module
                            .declare_function("molt_msgpack_parse_scalar", Linkage::Import, &sig)
                            .unwrap();
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
                        let mut err_sig = self.module.make_signature();
                        err_sig.params.push(AbiParam::new(types::I64));
                        err_sig.returns.push(AbiParam::new(types::I64));
                        let err_callee = self
                            .module
                            .declare_function(
                                "molt_msgpack_parse_scalar_obj",
                                Linkage::Import,
                                &err_sig,
                            )
                            .unwrap();
                        let err_local = self.module.declare_func_in_func(err_callee, builder.func);
                        let err_call = builder.ins().call(err_local, &[*arg_bits]);
                        let err_res = builder.inst_results(err_call)[0];
                        jump_block(&mut builder, merge_block, &[err_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        let res = builder.block_params(merge_block)[0];
                        def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                    } else {
                        let arg_bits =
                            var_get(&mut builder, &vars, arg_name).expect("Bytes arg not found");
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function(
                                "molt_msgpack_parse_scalar_obj",
                                Linkage::Import,
                                &sig,
                            )
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*arg_bits]);
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                    }
                }
                "cbor_parse" => {
                    let args = op.args.as_ref().unwrap();
                    let arg_name = &args[0];
                    if let Some(len) = var_get(&mut builder, &vars, &format!("{}_len", arg_name)) {
                        let ptr = var_get(&mut builder, &vars, &format!("{}_ptr", arg_name))
                            .or_else(|| var_get(&mut builder, &vars, arg_name))
                            .expect("Bytes ptr not found");

                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64)); // ptr
                        sig.params.push(AbiParam::new(types::I64)); // len
                        sig.params.push(AbiParam::new(types::I64)); // out ptr
                        sig.returns.push(AbiParam::new(types::I32)); // status

                        let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            8,
                            3,
                        ));
                        let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);

                        let callee = self
                            .module
                            .declare_function("molt_cbor_parse_scalar", Linkage::Import, &sig)
                            .unwrap();
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
                        let mut err_sig = self.module.make_signature();
                        err_sig.params.push(AbiParam::new(types::I64));
                        err_sig.returns.push(AbiParam::new(types::I64));
                        let err_callee = self
                            .module
                            .declare_function(
                                "molt_cbor_parse_scalar_obj",
                                Linkage::Import,
                                &err_sig,
                            )
                            .unwrap();
                        let err_local = self.module.declare_func_in_func(err_callee, builder.func);
                        let err_call = builder.ins().call(err_local, &[*arg_bits]);
                        let err_res = builder.inst_results(err_call)[0];
                        jump_block(&mut builder, merge_block, &[err_res]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        let res = builder.block_params(merge_block)[0];
                        def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                    } else {
                        let arg_bits =
                            var_get(&mut builder, &vars, arg_name).expect("Bytes arg not found");
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_cbor_parse_scalar_obj", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*arg_bits]);
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                    }
                }
                "block_on" => {
                    let args = op.args.as_ref().unwrap();
                    let task = var_get(&mut builder, &vars, &args[0]).expect("Task not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64)); // boxed task
                    sig.returns.push(AbiParam::new(types::I64));

                    let callee = self
                        .module
                        .declare_function("molt_block_on", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*task]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "state_switch" => {
                    let self_ptr = builder.block_params(entry_block)[0];
                    let state = builder.ins().load(
                        types::I64,
                        MemFlags::trusted(),
                        self_ptr,
                        HEADER_STATE_OFFSET,
                    );
                    let self_bits = box_ptr_value(&mut builder, self_ptr);
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
                    let args = op.args.as_ref().unwrap();
                    let future = var_get(&mut builder, &vars, &args[0]).expect("Future not found");
                    let future_ptr = unbox_ptr_value(&mut builder, *future);
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
                    let next_state_id = op.value.unwrap();
                    let self_bits = *var_get(&mut builder, &vars, "self").expect("Self not found");
                    let self_ptr = unbox_ptr_value(&mut builder, self_bits);

                    let pending_state_id = unbox_int(&mut builder, pending_state_bits);
                    builder.ins().store(
                        MemFlags::trusted(),
                        pending_state_id,
                        self_ptr,
                        HEADER_STATE_OFFSET,
                    );

                    let mut poll_sig = self.module.make_signature();
                    poll_sig.params.push(AbiParam::new(types::I64));
                    poll_sig.returns.push(AbiParam::new(types::I64));
                    let poll_callee = self
                        .module
                        .declare_function("molt_future_poll", Linkage::Import, &poll_sig)
                        .unwrap();
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
                    let mut sleep_sig = self.module.make_signature();
                    sleep_sig.params.push(AbiParam::new(types::I64));
                    sleep_sig.params.push(AbiParam::new(types::I64));
                    sleep_sig.returns.push(AbiParam::new(types::I64));
                    let sleep_callee = self
                        .module
                        .declare_function("molt_sleep_register", Linkage::Import, &sleep_sig)
                        .unwrap();
                    let local_sleep = self.module.declare_func_in_func(sleep_callee, builder.func);
                    builder.ins().call(local_sleep, &[self_ptr, future_ptr]);
                    reachable_blocks.insert(master_return_block);
                    jump_block(&mut builder, master_return_block, &[pending_const]);

                    switch_to_block_tracking(&mut builder, ready_path, &mut is_block_filled);
                    builder.seal_block(ready_path);
                    if let Some(bits) = slot_bits {
                        let offset = unbox_int(&mut builder, bits);
                        let mut store_sig = self.module.make_signature();
                        store_sig.params.push(AbiParam::new(types::I64));
                        store_sig.params.push(AbiParam::new(types::I64));
                        store_sig.params.push(AbiParam::new(types::I64));
                        store_sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_closure_store", Linkage::Import, &store_sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        builder.ins().call(local_callee, &[self_ptr, offset, res]);
                    }
                    let state_val = builder.ins().iconst(types::I64, next_state_id);
                    builder.ins().store(
                        MemFlags::trusted(),
                        state_val,
                        self_ptr,
                        HEADER_STATE_OFFSET,
                    );
                    if args.len() <= 1 {
                        def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                    }
                    jump_block(&mut builder, next_block, &[]);

                    switch_to_block_tracking(&mut builder, next_block, &mut is_block_filled);
                }
                "state_yield" => {
                    let args = op.args.as_ref().unwrap();
                    let pair =
                        var_get(&mut builder, &vars, &args[0]).expect("Yield pair not found");
                    let next_state_id = op.value.unwrap();
                    let self_bits = *var_get(&mut builder, &vars, "self").expect("Self not found");
                    let self_ptr = unbox_ptr_value(&mut builder, self_bits);

                    let state_val = builder.ins().iconst(types::I64, next_state_id);
                    builder.ins().store(
                        MemFlags::trusted(),
                        state_val,
                        self_ptr,
                        HEADER_STATE_OFFSET,
                    );

                    reachable_blocks.insert(master_return_block);
                    if has_ret {
                        // Suspension returns an owned value to the caller; explicitly
                        // retain it here so downstream cleanup/control-flow lowering cannot
                        // invalidate yielded data before next()/send()/throw() unwraps it.
                        emit_inc_ref_obj(&mut builder, *pair, local_inc_ref_obj);
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
                    let args = op.args.as_ref().unwrap();
                    let chan = var_get(&mut builder, &vars, &args[0]).expect("Chan not found");
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Val not found");
                    let pending_state_bits =
                        *var_get(&mut builder, &vars, &args[2]).expect("Pending state not found");
                    let next_state_id = op.value.unwrap();
                    let self_bits = *var_get(&mut builder, &vars, "self").expect("Self not found");
                    let self_ptr = unbox_ptr_value(&mut builder, self_bits);

                    let pending_state_id = unbox_int(&mut builder, pending_state_bits);
                    builder.ins().store(
                        MemFlags::trusted(),
                        pending_state_id,
                        self_ptr,
                        HEADER_STATE_OFFSET,
                    );

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_chan_send", Linkage::Import, &sig)
                        .unwrap();
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
                    builder.ins().store(
                        MemFlags::trusted(),
                        state_val,
                        self_ptr,
                        HEADER_STATE_OFFSET,
                    );
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                    reachable_blocks.insert(next_block);
                    jump_block(&mut builder, next_block, &[]);

                    if reachable_blocks.contains(&next_block) {
                        switch_to_block_tracking(&mut builder, next_block, &mut is_block_filled);
                    } else {
                        is_block_filled = true;
                    }
                }
                "chan_recv_yield" => {
                    let args = op.args.as_ref().unwrap();
                    let chan = var_get(&mut builder, &vars, &args[0]).expect("Chan not found");
                    let pending_state_bits =
                        *var_get(&mut builder, &vars, &args[1]).expect("Pending state not found");
                    let next_state_id = op.value.unwrap();
                    let self_bits = *var_get(&mut builder, &vars, "self").expect("Self not found");
                    let self_ptr = unbox_ptr_value(&mut builder, self_bits);

                    let pending_state_id = unbox_int(&mut builder, pending_state_bits);
                    builder.ins().store(
                        MemFlags::trusted(),
                        pending_state_id,
                        self_ptr,
                        HEADER_STATE_OFFSET,
                    );

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_chan_recv", Linkage::Import, &sig)
                        .unwrap();
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
                    builder.ins().store(
                        MemFlags::trusted(),
                        state_val,
                        self_ptr,
                        HEADER_STATE_OFFSET,
                    );
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                    reachable_blocks.insert(next_block);
                    jump_block(&mut builder, next_block, &[]);

                    if reachable_blocks.contains(&next_block) {
                        switch_to_block_tracking(&mut builder, next_block, &mut is_block_filled);
                    } else {
                        is_block_filled = true;
                    }
                }
                "chan_new" => {
                    let args = op.args.as_ref().unwrap();
                    let capacity =
                        var_get(&mut builder, &vars, &args[0]).expect("Capacity not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_chan_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*capacity]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "chan_drop" => {
                    let args = op.args.as_ref().unwrap();
                    let chan = var_get(&mut builder, &vars, &args[0]).expect("Chan not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_chan_drop", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*chan]);
                    let _ = builder.inst_results(call)[0];
                }
                "spawn" => {
                    let args = op.args.as_ref().unwrap();
                    let task = var_get(&mut builder, &vars, &args[0]).expect("Task not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_spawn", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*task]);
                }
                "cancel_token_new" => {
                    let args = op.args.as_ref().unwrap();
                    let parent =
                        var_get(&mut builder, &vars, &args[0]).expect("Parent token not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*parent]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "cancel_token_clone" => {
                    let args = op.args.as_ref().unwrap();
                    let token = var_get(&mut builder, &vars, &args[0]).expect("Token not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_clone", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*token]);
                }
                "cancel_token_drop" => {
                    let args = op.args.as_ref().unwrap();
                    let token = var_get(&mut builder, &vars, &args[0]).expect("Token not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_drop", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*token]);
                }
                "cancel_token_cancel" => {
                    let args = op.args.as_ref().unwrap();
                    let token = var_get(&mut builder, &vars, &args[0]).expect("Token not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_cancel", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*token]);
                }
                "future_cancel" => {
                    let args = op.args.as_ref().unwrap();
                    let future = var_get(&mut builder, &vars, &args[0]).expect("Future not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_future_cancel", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*future]);
                }
                "future_cancel_msg" => {
                    let args = op.args.as_ref().unwrap();
                    let future = var_get(&mut builder, &vars, &args[0]).expect("Future not found");
                    let msg =
                        var_get(&mut builder, &vars, &args[1]).expect("Cancel message not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_future_cancel_msg", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*future, *msg]);
                }
                "future_cancel_clear" => {
                    let args = op.args.as_ref().unwrap();
                    let future = var_get(&mut builder, &vars, &args[0]).expect("Future not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_future_cancel_clear", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*future]);
                }
                "promise_new" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_promise_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "promise_set_result" => {
                    let args = op.args.as_ref().unwrap();
                    let future = var_get(&mut builder, &vars, &args[0]).expect("Promise not found");
                    let result = var_get(&mut builder, &vars, &args[1]).expect("Result not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_promise_set_result", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*future, *result]);
                }
                "promise_set_exception" => {
                    let args = op.args.as_ref().unwrap();
                    let future = var_get(&mut builder, &vars, &args[0]).expect("Promise not found");
                    let exc = var_get(&mut builder, &vars, &args[1]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_promise_set_exception", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*future, *exc]);
                }
                "thread_submit" => {
                    let args = op.args.as_ref().unwrap();
                    let callable =
                        var_get(&mut builder, &vars, &args[0]).expect("Callable not found");
                    let call_args = var_get(&mut builder, &vars, &args[1]).expect("Args not found");
                    let call_kwargs =
                        var_get(&mut builder, &vars, &args[2]).expect("Kwargs not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_thread_submit", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*callable, *call_args, *call_kwargs]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "task_register_token_owned" => {
                    let args = op.args.as_ref().unwrap();
                    let task = var_get(&mut builder, &vars, &args[0]).expect("Task not found");
                    let token = var_get(&mut builder, &vars, &args[1]).expect("Token not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_task_register_token_owned", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*task, *token]);
                }
                "cancel_token_is_cancelled" => {
                    let args = op.args.as_ref().unwrap();
                    let token = var_get(&mut builder, &vars, &args[0]).expect("Token not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_is_cancelled", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*token]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "cancel_token_set_current" => {
                    let args = op.args.as_ref().unwrap();
                    let token = var_get(&mut builder, &vars, &args[0]).expect("Token not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_set_current", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*token]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "cancel_token_get_current" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_token_get_current", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "cancelled" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancelled", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "cancel_current" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_cancel_current", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[]);
                }
                "call_async" => {
                    let poll_func_name = op.s_value.as_ref().unwrap();
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
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_async_sleep_new", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[delay_val, result_val]);
                        let res = builder.inst_results(call)[0];
                        let out_name = op.out.unwrap();
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

                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let task_callee = self
                            .module
                            .declare_function("molt_task_new", Linkage::Import, &sig)
                            .unwrap();
                        let task_local =
                            self.module.declare_func_in_func(task_callee, builder.func);
                        let kind_val = builder.ins().iconst(types::I64, TASK_KIND_FUTURE);
                        let call = builder.ins().call(task_local, &[poll_addr, size, kind_val]);
                        let obj = builder.inst_results(call)[0];
                        let obj_ptr = unbox_ptr_value(&mut builder, obj);

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
                                emit_inc_ref_obj(&mut builder, *val, local_inc_ref_obj);
                            }
                        }
                        let out_name = op.out.unwrap();
                        def_var_named(&mut builder, &vars, out_name, obj);
                    }
                }
                "builtin_func" => {
                    let func_name = op.s_value.as_ref().unwrap();
                    let arity = op.value.unwrap();
                    let mut func_sig = self.module.make_signature();
                    for _ in 0..arity {
                        func_sig.params.push(AbiParam::new(types::I64));
                    }
                    func_sig.returns.push(AbiParam::new(types::I64));
                    let func_id = self
                        .module
                        .declare_function(func_name, Linkage::Import, &func_sig)
                        .unwrap();
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let tramp_id = Self::ensure_trampoline(
                        &mut self.module,
                        &mut self.trampoline_ids,
                        func_name,
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

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_func_new_builtin", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[func_addr, tramp_addr, arity_val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "func_new" => {
                    let func_name = op.s_value.as_ref().unwrap();
                    let arity = op.value.unwrap();
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
                    let func_id = self
                        .module
                        .declare_function(func_name, Linkage::Export, &func_sig)
                        .unwrap();
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let tramp_id = Self::ensure_trampoline(
                        &mut self.module,
                        &mut self.trampoline_ids,
                        func_name,
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

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_func_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[func_addr, tramp_addr, arity_val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "func_new_closure" => {
                    let func_name = op.s_value.as_ref().unwrap();
                    let arity = op.value.unwrap();
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
                    let func_id = self
                        .module
                        .declare_function(func_name, Linkage::Export, &func_sig)
                        .unwrap();
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let tramp_id = Self::ensure_trampoline(
                        &mut self.module,
                        &mut self.trampoline_ids,
                        func_name,
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

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_func_new_closure", Linkage::Import, &sig)
                        .unwrap();
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "code_new" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_code_new", Linkage::Import, &sig)
                        .unwrap();
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "code_slot_set" => {
                    let args = op.args.as_ref().unwrap();
                    let code_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("code bits not found");
                    let code_id = op.value.unwrap_or(0);
                    let code_id_val = builder.ins().iconst(types::I64, code_id);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_code_slot_set", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder.ins().call(local_callee, &[code_id_val, *code_bits]);
                }
                "fn_ptr_code_set" => {
                    let args = op.args.as_ref().unwrap();
                    let code_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("code bits not found");
                    let func_name = op.s_value.as_ref().expect("fn_ptr_code_set expects symbol");
                    let mut func_sig = self.module.make_signature();
                    if func_name.ends_with("_poll") {
                        func_sig.params.push(AbiParam::new(types::I64));
                    } else {
                        let arity = op.value.unwrap_or(0);
                        for _ in 0..arity {
                            func_sig.params.push(AbiParam::new(types::I64));
                        }
                    }
                    func_sig.returns.push(AbiParam::new(types::I64));
                    let func_id = self
                        .module
                        .declare_function(func_name, Linkage::Export, &func_sig)
                        .unwrap();
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_fn_ptr_code_set", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder.ins().call(local_callee, &[func_addr, *code_bits]);
                }
                "asyncgen_locals_register" => {
                    let args = op.args.as_ref().unwrap();
                    let names_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("names tuple not found");
                    let offsets_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("offsets tuple not found");
                    let func_name = op
                        .s_value
                        .as_ref()
                        .expect("asyncgen_locals_register expects symbol");
                    let mut func_sig = self.module.make_signature();
                    if func_name.ends_with("_poll") {
                        func_sig.params.push(AbiParam::new(types::I64));
                    } else {
                        let arity = op.value.unwrap_or(0);
                        for _ in 0..arity {
                            func_sig.params.push(AbiParam::new(types::I64));
                        }
                    }
                    func_sig.returns.push(AbiParam::new(types::I64));
                    let func_id = self
                        .module
                        .declare_function(func_name, Linkage::Export, &func_sig)
                        .unwrap();
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_asyncgen_locals_register", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder
                        .ins()
                        .call(local_callee, &[func_addr, *names_bits, *offsets_bits]);
                }
                "gen_locals_register" => {
                    let args = op.args.as_ref().unwrap();
                    let names_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("names tuple not found");
                    let offsets_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("offsets tuple not found");
                    let func_name = op
                        .s_value
                        .as_ref()
                        .expect("gen_locals_register expects symbol");
                    let mut func_sig = self.module.make_signature();
                    if func_name.ends_with("_poll") {
                        func_sig.params.push(AbiParam::new(types::I64));
                    } else {
                        let arity = op.value.unwrap_or(0);
                        for _ in 0..arity {
                            func_sig.params.push(AbiParam::new(types::I64));
                        }
                    }
                    func_sig.returns.push(AbiParam::new(types::I64));
                    let func_id = self
                        .module
                        .declare_function(func_name, Linkage::Export, &func_sig)
                        .unwrap();
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    let func_addr = builder.ins().func_addr(types::I64, func_ref);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_gen_locals_register", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder
                        .ins()
                        .call(local_callee, &[func_addr, *names_bits, *offsets_bits]);
                }
                "code_slots_init" => {
                    let count = op.value.unwrap_or(0);
                    let count_val = builder.ins().iconst(types::I64, count);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_code_slots_init", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder.ins().call(local_callee, &[count_val]);
                }
                "trace_enter_slot" => {
                    if emit_traces {
                        let code_id = op.value.unwrap_or(0);
                        let code_id_val = builder.ins().iconst(types::I64, code_id);
                        let mut sig = self.module.make_signature();
                        sig.params.push(AbiParam::new(types::I64));
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_trace_enter_slot", Linkage::Import, &sig)
                            .unwrap();
                        let local_callee = self.module.declare_func_in_func(callee, builder.func);
                        let _ = builder.ins().call(local_callee, &[code_id_val]);
                    }
                }
                "trace_exit" => {
                    if emit_traces {
                        let mut sig = self.module.make_signature();
                        sig.returns.push(AbiParam::new(types::I64));
                        let callee = self
                            .module
                            .declare_function("molt_trace_exit", Linkage::Import, &sig)
                            .unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_frame_locals_set", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder.ins().call(local_callee, &[dict_bits]);
                }
                "line" => {
                    let line = op.value.unwrap_or(0);
                    let line_val = builder.ins().iconst(types::I64, line);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_trace_set_line", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let _ = builder.ins().call(local_callee, &[line_val]);
                    if !is_block_filled && let Some(block) = builder.current_block() {
                        if let Some(names) = block_tracked_obj.get_mut(&block) {
                            let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
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
                        }
                        if let Some(names) = block_tracked_ptr.get_mut(&block) {
                            let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                            for name in cleanup {
                                let val =
                                    var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                        panic!(
                                            "Tracked ptr var not found in {} op {}: {}",
                                            func_ir.name, op_idx, name
                                        )
                                    });
                                builder.ins().call(local_dec_ref, &[*val]);
                            }
                        }
                    }
                }
                "missing" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_missing", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "function_closure_bits" => {
                    let args = op.args.as_ref().unwrap();
                    let func_bits = var_get(&mut builder, &vars, &args[0]).expect("Func not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_function_closure_bits", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*func_bits]);
                    let res = builder.inst_results(call)[0];
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj);
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bound_method_new" => {
                    let args = op.args.as_ref().unwrap();
                    let func_bits = var_get(&mut builder, &vars, &args[0]).expect("Func not found");
                    let self_bits = var_get(&mut builder, &vars, &args[1]).expect("Self not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bound_method_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*func_bits, *self_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "call" => {
                    let target_name = op.s_value.as_ref().unwrap();
                    let args_names = op.args.as_ref().unwrap();
                    let mut args = Vec::new();
                    for name in args_names {
                        args.push(*var_get(&mut builder, &vars, name).expect("Arg not found"));
                    }

                    // Collect arg values that are dead after this call. We explicitly avoid
                    // decrementing function parameters here: parameters are treated as borrowed
                    // by this backend (caller owns), so only non-param temporaries should be
                    // released at the call site.
                    let mut arg_cleanup = Vec::new();
                    let mut arg_cleanup_names = HashSet::new();
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
                        && (func_ir.name.contains("open_arg_drop_check")
                            || func_ir.name.contains("builtins_symbol_open"))
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
                        "molt_trace_enter_slot",
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
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);
                    // Carry any live tracked values across the call's internal control flow into the
                    // continuation block.
                    if !origin_obj_live.is_empty() {
                        extend_unique_tracked(
                            block_tracked_obj.entry(merge_block).or_default(),
                            origin_obj_live.clone(),
                        );
                    }
                    if !origin_ptr_live.is_empty() {
                        extend_unique_tracked(
                            block_tracked_ptr.entry(merge_block).or_default(),
                            origin_ptr_live.clone(),
                        );
                    }
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let call_block = builder.create_block();
                    let fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, call_block, &[], fail_block, &[]);

                    builder.switch_to_block(call_block);
                    builder.seal_block(call_block);
                    if emit_traces {
                        let code_id = op.value.unwrap_or(0);
                        let code_id_val = builder.ins().iconst(types::I64, code_id);
                        let _ = builder.ins().call(trace_enter_local, &[code_id_val]);
                    }
                    let call = builder.ins().call(local_callee, &args);
                    let res = builder.inst_results(call)[0];
                    if emit_traces {
                        let _ = builder.ins().call(trace_exit_local, &[]);
                    }
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    for name in &origin_obj_cleanup {
                        if arg_cleanup_names.contains(name) {
                            continue;
                        }
                        let val = var_get(&mut builder, &vars, name).unwrap_or_else(|| {
                            panic!(
                                "Tracked obj var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                    for name in &origin_ptr_cleanup {
                        let val = var_get(&mut builder, &vars, name).unwrap_or_else(|| {
                            panic!(
                                "Tracked ptr var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                        builder.ins().call(local_dec_ref, &[*val]);
                    }
                    for val in &arg_cleanup {
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                    jump_block(&mut builder, merge_block, &[res]);

                    builder.switch_to_block(fail_block);
                    builder.seal_block(fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    for name in &origin_obj_cleanup {
                        if arg_cleanup_names.contains(name) {
                            continue;
                        }
                        let val = var_get(&mut builder, &vars, name).unwrap_or_else(|| {
                            panic!(
                                "Tracked obj var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                    for name in &origin_ptr_cleanup {
                        let val = var_get(&mut builder, &vars, name).unwrap_or_else(|| {
                            panic!(
                                "Tracked ptr var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                        builder.ins().call(local_dec_ref, &[*val]);
                    }
                    for val in &arg_cleanup {
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                    jump_block(&mut builder, merge_block, &[none_bits]);

                    builder.switch_to_block(merge_block);
                    builder.seal_block(merge_block);
                    let res = builder.block_params(merge_block)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "call_internal" => {
                    let target_name = op.s_value.as_ref().unwrap();
                    let args_names = op.args.as_ref().unwrap();
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "inc_ref" | "borrow" => {
                    let args_names = op.args.as_ref().expect("inc_ref/borrow args missing");
                    let src_name = args_names
                        .first()
                        .expect("inc_ref/borrow requires one source arg");
                    let src = *var_get(&mut builder, &vars, src_name)
                        .expect("inc_ref/borrow source not found");
                    emit_inc_ref_obj(&mut builder, src, local_inc_ref_obj);
                    if let Some(out_name) = op.out.as_ref()
                        && out_name != "none"
                    {
                        def_var_named(&mut builder, &vars, out_name.clone(), src);
                    }
                }
                "dec_ref" | "release" => {
                    let args_names = op.args.as_ref().expect("dec_ref/release args missing");
                    let src_name = args_names
                        .first()
                        .expect("dec_ref/release requires one source arg");
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
                        emit_inc_ref_obj(&mut builder, src, local_inc_ref_obj);
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
                        emit_inc_ref_obj(&mut builder, src, local_inc_ref_obj);
                        def_var_named(&mut builder, &vars, out_name.clone(), src);
                    }
                }
                "call_guarded" => {
                    let target_name = op.s_value.as_ref().unwrap();
                    let args_names = op.args.as_ref().unwrap();
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
                        let mut extract_sig = self.module.make_signature();
                        extract_sig.params.push(AbiParam::new(types::I64));
                        extract_sig.returns.push(AbiParam::new(types::I64));
                        let extract_fn = self
                            .module
                            .declare_function(
                                "molt_function_closure_bits",
                                Linkage::Import,
                                &extract_sig,
                            )
                            .unwrap();
                        let extract_local =
                            self.module.declare_func_in_func(extract_fn, builder.func);
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "call_func" => {
                    let args_names = op.args.as_ref().unwrap();
                    let func_bits =
                        var_get(&mut builder, &vars, &args_names[0]).expect("Func not found");
                    let mut args = Vec::new();
                    for name in &args_names[1..] {
                        args.push(*var_get(&mut builder, &vars, name).expect("Arg not found"));
                    }
                    let call_site_prefix = "call_func";

                    let resolve_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_handle_resolve",
                        &[types::I64],
                        &[types::I64],
                    );
                    let is_bound_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_is_bound_method",
                        &[types::I64],
                        &[types::I64],
                    );
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
                    let default_kind_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_function_default_kind",
                        &[types::I64],
                        &[types::I64],
                    );
                    let closure_bits_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_function_closure_bits",
                        &[types::I64],
                        &[types::I64],
                    );
                    let is_generator_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_function_is_generator",
                        &[types::I64],
                        &[types::I64],
                    );
                    let is_coroutine_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_function_is_coroutine",
                        &[types::I64],
                        &[types::I64],
                    );
                    let missing_local = import_func_ref(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        "molt_missing",
                        &[],
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
                    let is_bound_call = builder.ins().call(is_bound_local, &[*func_bits]);
                    let is_bound_bits = builder.inst_results(is_bound_call)[0];
                    let truthy_call = builder.ins().call(truthy_local, &[is_bound_bits]);
                    let truthy_bits = builder.inst_results(truthy_call)[0];
                    let is_bound_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy_bits, 0);

                    let bound_block = builder.create_block();
                    let non_bound_block = builder.create_block();
                    let func_block = builder.create_block();
                    let fallback_block = builder.create_block();
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);
                    builder
                        .ins()
                        .brif(is_bound_bool, bound_block, &[], non_bound_block, &[]);

                    builder.switch_to_block(bound_block);
                    builder.seal_block(bound_block);
                    let method_resolve = builder.ins().call(resolve_local, &[*func_bits]);
                    let method_ptr = builder.inst_results(method_resolve)[0];
                    let bound_func_bits =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::trusted(), method_ptr, 0);
                    let self_bits =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::trusted(), method_ptr, 8);
                    let bound_resolve = builder.ins().call(resolve_local, &[bound_func_bits]);
                    let bound_func_ptr = builder.inst_results(bound_resolve)[0];
                    let bound_fn_ptr =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::trusted(), bound_func_ptr, 0);
                    let closure_bits_call =
                        builder.ins().call(closure_bits_local, &[bound_func_bits]);
                    let closure_bits_val = builder.inst_results(closure_bits_call)[0];
                    let closure_is_zero = builder.ins().icmp_imm(IntCC::Equal, closure_bits_val, 0);
                    let is_gen_call = builder.ins().call(is_generator_local, &[bound_func_bits]);
                    let is_gen_bits = builder.inst_results(is_gen_call)[0];
                    let is_gen_truthy_call = builder.ins().call(truthy_local, &[is_gen_bits]);
                    let is_gen_truthy_bits = builder.inst_results(is_gen_truthy_call)[0];
                    let is_gen_bool =
                        builder
                            .ins()
                            .icmp_imm(IntCC::NotEqual, is_gen_truthy_bits, 0);
                    let is_coro_call = builder.ins().call(is_coroutine_local, &[bound_func_bits]);
                    let is_coro_bits = builder.inst_results(is_coro_call)[0];
                    let is_coro_truthy_call = builder.ins().call(truthy_local, &[is_coro_bits]);
                    let is_coro_truthy_bits = builder.inst_results(is_coro_truthy_call)[0];
                    let is_coro_bool =
                        builder
                            .ins()
                            .icmp_imm(IntCC::NotEqual, is_coro_truthy_bits, 0);
                    let bound_direct_block = builder.create_block();
                    let bound_closure_block = builder.create_block();
                    let bound_non_gen_block = builder.create_block();
                    let bound_non_special_block = builder.create_block();
                    brif_block(
                        &mut builder,
                        is_gen_bool,
                        bound_closure_block,
                        &[],
                        bound_non_gen_block,
                        &[],
                    );

                    builder.switch_to_block(bound_non_gen_block);
                    builder.seal_block(bound_non_gen_block);
                    brif_block(
                        &mut builder,
                        is_coro_bool,
                        bound_closure_block,
                        &[],
                        bound_non_special_block,
                        &[],
                    );

                    builder.switch_to_block(bound_non_special_block);
                    builder.seal_block(bound_non_special_block);
                    brif_block(
                        &mut builder,
                        closure_is_zero,
                        bound_direct_block,
                        &[],
                        bound_closure_block,
                        &[],
                    );

                    builder.switch_to_block(bound_closure_block);
                    builder.seal_block(bound_closure_block);
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
                    let bound_closure_label = format!("{call_site_prefix}_bound_closure");
                    let site_bits = builder.ins().iconst(
                        types::I64,
                        box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            bound_closure_label.as_str(),
                        )),
                    );
                    let bound_call = builder
                        .ins()
                        .call(call_bind_local, &[site_bits, *func_bits, callargs_ptr]);
                    let bound_res = builder.inst_results(bound_call)[0];
                    jump_block(&mut builder, merge_block, &[bound_res]);

                    builder.switch_to_block(bound_direct_block);
                    builder.seal_block(bound_direct_block);
                    let bound_arity =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::trusted(), bound_func_ptr, 8);
                    let provided_arity = builder.ins().iconst(types::I64, (args.len() + 1) as i64);
                    let missing = builder.ins().isub(bound_arity, provided_arity);
                    let zero = builder.ins().iconst(types::I64, 0);
                    let one = builder.ins().iconst(types::I64, 1);
                    let two = builder.ins().iconst(types::I64, 2);
                    let is_zero = builder.ins().icmp(IntCC::Equal, missing, zero);
                    let is_one = builder.ins().icmp(IntCC::Equal, missing, one);
                    let is_two = builder.ins().icmp(IntCC::Equal, missing, two);
                    let default_kind_call =
                        builder.ins().call(default_kind_local, &[bound_func_bits]);
                    let default_kind_val = builder.inst_results(default_kind_call)[0];
                    let default_none = builder.ins().iconst(types::I64, FUNC_DEFAULT_NONE);
                    let default_pop = builder.ins().iconst(types::I64, FUNC_DEFAULT_DICT_POP);
                    let default_update = builder.ins().iconst(types::I64, FUNC_DEFAULT_DICT_UPDATE);

                    let bound_exact_block = builder.create_block();
                    let bound_missing_one_block = builder.create_block();
                    let bound_missing_two_block = builder.create_block();
                    let bound_error_block = builder.create_block();
                    let bound_missing_check = builder.create_block();
                    let bound_missing_two_check = builder.create_block();

                    builder
                        .ins()
                        .brif(is_zero, bound_exact_block, &[], bound_missing_check, &[]);

                    builder.switch_to_block(bound_missing_check);
                    builder.seal_block(bound_missing_check);
                    brif_block(
                        &mut builder,
                        is_one,
                        bound_missing_one_block,
                        &[],
                        bound_missing_two_check,
                        &[],
                    );

                    builder.switch_to_block(bound_missing_two_check);
                    builder.seal_block(bound_missing_two_check);
                    brif_block(
                        &mut builder,
                        is_two,
                        bound_missing_two_block,
                        &[],
                        bound_error_block,
                        &[],
                    );

                    builder.switch_to_block(bound_exact_block);
                    builder.seal_block(bound_exact_block);
                    let mut bound_args = Vec::with_capacity(args.len() + 1);
                    bound_args.push(self_bits);
                    bound_args.extend(args.iter().copied());
                    let mut bound_sig = self.module.make_signature();
                    for _ in 0..bound_args.len() {
                        bound_sig.params.push(AbiParam::new(types::I64));
                    }
                    bound_sig.returns.push(AbiParam::new(types::I64));
                    let bound_sig_ref = builder.import_signature(bound_sig);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let bound_call_block = builder.create_block();
                    let bound_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, bound_call_block, &[], bound_fail_block, &[]);

                    builder.switch_to_block(bound_call_block);
                    builder.seal_block(bound_call_block);
                    if emit_traces {
                        let _ = builder.ins().call(trace_enter_local, &[bound_func_bits]);
                    }
                    let bound_call =
                        builder
                            .ins()
                            .call_indirect(bound_sig_ref, bound_fn_ptr, &bound_args);
                    let bound_res = builder.inst_results(bound_call)[0];
                    if emit_traces {
                        let _ = builder.ins().call(trace_exit_local, &[]);
                    }
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    jump_block(&mut builder, merge_block, &[bound_res]);

                    builder.switch_to_block(bound_fail_block);
                    builder.seal_block(bound_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    jump_block(&mut builder, merge_block, &[none_bits]);

                    builder.switch_to_block(bound_missing_one_block);
                    builder.seal_block(bound_missing_one_block);
                    let is_default_none =
                        builder
                            .ins()
                            .icmp(IntCC::Equal, default_kind_val, default_none);
                    let is_default_pop =
                        builder
                            .ins()
                            .icmp(IntCC::Equal, default_kind_val, default_pop);
                    let is_default_update =
                        builder
                            .ins()
                            .icmp(IntCC::Equal, default_kind_val, default_update);
                    let bound_missing_one_default = builder.create_block();
                    let bound_missing_one_pop = builder.create_block();
                    let bound_missing_one_update = builder.create_block();
                    let bound_missing_one_check = builder.create_block();
                    brif_block(
                        &mut builder,
                        is_default_none,
                        bound_missing_one_default,
                        &[],
                        bound_missing_one_check,
                        &[],
                    );

                    builder.switch_to_block(bound_missing_one_check);
                    builder.seal_block(bound_missing_one_check);
                    brif_block(
                        &mut builder,
                        is_default_pop,
                        bound_missing_one_pop,
                        &[],
                        bound_missing_one_update,
                        &[],
                    );

                    builder.switch_to_block(bound_missing_one_default);
                    builder.seal_block(bound_missing_one_default);
                    let mut bound_args = Vec::with_capacity(args.len() + 2);
                    bound_args.push(self_bits);
                    bound_args.extend(args.iter().copied());
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    bound_args.push(none_bits);
                    let mut bound_sig = self.module.make_signature();
                    for _ in 0..bound_args.len() {
                        bound_sig.params.push(AbiParam::new(types::I64));
                    }
                    bound_sig.returns.push(AbiParam::new(types::I64));
                    let bound_sig_ref = builder.import_signature(bound_sig);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let bound_call_block = builder.create_block();
                    let bound_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, bound_call_block, &[], bound_fail_block, &[]);

                    builder.switch_to_block(bound_call_block);
                    builder.seal_block(bound_call_block);
                    if emit_traces {
                        let _ = builder.ins().call(trace_enter_local, &[bound_func_bits]);
                    }
                    let bound_call =
                        builder
                            .ins()
                            .call_indirect(bound_sig_ref, bound_fn_ptr, &bound_args);
                    let bound_res = builder.inst_results(bound_call)[0];
                    if emit_traces {
                        let _ = builder.ins().call(trace_exit_local, &[]);
                    }
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    jump_block(&mut builder, merge_block, &[bound_res]);

                    builder.switch_to_block(bound_fail_block);
                    builder.seal_block(bound_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    jump_block(&mut builder, merge_block, &[none_bits]);

                    builder.switch_to_block(bound_missing_one_pop);
                    builder.seal_block(bound_missing_one_pop);
                    let mut bound_args = Vec::with_capacity(args.len() + 2);
                    bound_args.push(self_bits);
                    bound_args.extend(args.iter().copied());
                    let has_default_bits = builder.ins().iconst(types::I64, box_int(1));
                    bound_args.push(has_default_bits);
                    let mut bound_sig = self.module.make_signature();
                    for _ in 0..bound_args.len() {
                        bound_sig.params.push(AbiParam::new(types::I64));
                    }
                    bound_sig.returns.push(AbiParam::new(types::I64));
                    let bound_sig_ref = builder.import_signature(bound_sig);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let bound_call_block = builder.create_block();
                    let bound_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, bound_call_block, &[], bound_fail_block, &[]);

                    builder.switch_to_block(bound_call_block);
                    builder.seal_block(bound_call_block);
                    if emit_traces {
                        let _ = builder.ins().call(trace_enter_local, &[bound_func_bits]);
                    }
                    let bound_call =
                        builder
                            .ins()
                            .call_indirect(bound_sig_ref, bound_fn_ptr, &bound_args);
                    let bound_res = builder.inst_results(bound_call)[0];
                    if emit_traces {
                        let _ = builder.ins().call(trace_exit_local, &[]);
                    }
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    jump_block(&mut builder, merge_block, &[bound_res]);

                    builder.switch_to_block(bound_fail_block);
                    builder.seal_block(bound_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    jump_block(&mut builder, merge_block, &[none_bits]);

                    builder.switch_to_block(bound_missing_one_update);
                    builder.seal_block(bound_missing_one_update);
                    let bound_missing_one_update_ok = builder.create_block();
                    brif_block(
                        &mut builder,
                        is_default_update,
                        bound_missing_one_update_ok,
                        &[],
                        bound_error_block,
                        &[],
                    );

                    builder.switch_to_block(bound_missing_one_update_ok);
                    builder.seal_block(bound_missing_one_update_ok);
                    let missing_call = builder.ins().call(missing_local, &[]);
                    let missing_bits = builder.inst_results(missing_call)[0];
                    let mut bound_args = Vec::with_capacity(args.len() + 2);
                    bound_args.push(self_bits);
                    bound_args.extend(args.iter().copied());
                    bound_args.push(missing_bits);
                    let mut bound_sig = self.module.make_signature();
                    for _ in 0..bound_args.len() {
                        bound_sig.params.push(AbiParam::new(types::I64));
                    }
                    bound_sig.returns.push(AbiParam::new(types::I64));
                    let bound_sig_ref = builder.import_signature(bound_sig);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let bound_call_block = builder.create_block();
                    let bound_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, bound_call_block, &[], bound_fail_block, &[]);

                    builder.switch_to_block(bound_call_block);
                    builder.seal_block(bound_call_block);
                    if emit_traces {
                        let _ = builder.ins().call(trace_enter_local, &[bound_func_bits]);
                    }
                    let bound_call =
                        builder
                            .ins()
                            .call_indirect(bound_sig_ref, bound_fn_ptr, &bound_args);
                    let bound_res = builder.inst_results(bound_call)[0];
                    if emit_traces {
                        let _ = builder.ins().call(trace_exit_local, &[]);
                    }
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    jump_block(&mut builder, merge_block, &[bound_res]);

                    builder.switch_to_block(bound_fail_block);
                    builder.seal_block(bound_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    jump_block(&mut builder, merge_block, &[none_bits]);

                    builder.switch_to_block(bound_missing_two_block);
                    builder.seal_block(bound_missing_two_block);
                    let is_default_pop =
                        builder
                            .ins()
                            .icmp(IntCC::Equal, default_kind_val, default_pop);
                    let bound_missing_two_ok = builder.create_block();
                    brif_block(
                        &mut builder,
                        is_default_pop,
                        bound_missing_two_ok,
                        &[],
                        bound_error_block,
                        &[],
                    );

                    builder.switch_to_block(bound_missing_two_ok);
                    builder.seal_block(bound_missing_two_ok);
                    let mut bound_args = Vec::with_capacity(args.len() + 3);
                    bound_args.push(self_bits);
                    bound_args.extend(args.iter().copied());
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    let has_default_bits = builder.ins().iconst(types::I64, box_int(0));
                    bound_args.push(none_bits);
                    bound_args.push(has_default_bits);
                    let mut bound_sig = self.module.make_signature();
                    for _ in 0..bound_args.len() {
                        bound_sig.params.push(AbiParam::new(types::I64));
                    }
                    bound_sig.returns.push(AbiParam::new(types::I64));
                    let bound_sig_ref = builder.import_signature(bound_sig);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let bound_call_block = builder.create_block();
                    let bound_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, bound_call_block, &[], bound_fail_block, &[]);

                    builder.switch_to_block(bound_call_block);
                    builder.seal_block(bound_call_block);
                    if emit_traces {
                        let _ = builder.ins().call(trace_enter_local, &[bound_func_bits]);
                    }
                    let bound_call =
                        builder
                            .ins()
                            .call_indirect(bound_sig_ref, bound_fn_ptr, &bound_args);
                    let bound_res = builder.inst_results(bound_call)[0];
                    if emit_traces {
                        let _ = builder.ins().call(trace_exit_local, &[]);
                    }
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    jump_block(&mut builder, merge_block, &[bound_res]);

                    builder.switch_to_block(bound_fail_block);
                    builder.seal_block(bound_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    jump_block(&mut builder, merge_block, &[none_bits]);

                    builder.switch_to_block(bound_error_block);
                    builder.seal_block(bound_error_block);
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
                    let bound_error_label = format!("{call_site_prefix}_bound_error");
                    let site_bits = builder.ins().iconst(
                        types::I64,
                        box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            bound_error_label.as_str(),
                        )),
                    );
                    let fallback_call = builder
                        .ins()
                        .call(call_bind_local, &[site_bits, *func_bits, callargs_ptr]);
                    let fallback_res = builder.inst_results(fallback_call)[0];
                    jump_block(&mut builder, merge_block, &[fallback_res]);

                    builder.switch_to_block(non_bound_block);
                    builder.seal_block(non_bound_block);
                    let is_func_call = builder.ins().call(is_func_local, &[*func_bits]);
                    let is_func_bits = builder.inst_results(is_func_call)[0];
                    let truthy_call = builder.ins().call(truthy_local, &[is_func_bits]);
                    let truthy_bits = builder.inst_results(truthy_call)[0];
                    let is_func_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy_bits, 0);
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
                    let nonfunc_fallback_label = format!("{call_site_prefix}_nonfunc_fallback");
                    let site_bits = builder.ins().iconst(
                        types::I64,
                        box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            nonfunc_fallback_label.as_str(),
                        )),
                    );
                    let fallback_call = builder
                        .ins()
                        .call(call_bind_local, &[site_bits, *func_bits, callargs_ptr]);
                    let fallback_res = builder.inst_results(fallback_call)[0];
                    jump_block(&mut builder, merge_block, &[fallback_res]);

                    builder.switch_to_block(func_block);
                    builder.seal_block(func_block);
                    let closure_bits_call = builder.ins().call(closure_bits_local, &[*func_bits]);
                    let closure_bits_val = builder.inst_results(closure_bits_call)[0];
                    let closure_is_zero = builder.ins().icmp_imm(IntCC::Equal, closure_bits_val, 0);
                    let is_gen_call = builder.ins().call(is_generator_local, &[*func_bits]);
                    let is_gen_bits = builder.inst_results(is_gen_call)[0];
                    let is_gen_truthy_call = builder.ins().call(truthy_local, &[is_gen_bits]);
                    let is_gen_truthy_bits = builder.inst_results(is_gen_truthy_call)[0];
                    let is_gen_bool =
                        builder
                            .ins()
                            .icmp_imm(IntCC::NotEqual, is_gen_truthy_bits, 0);
                    let is_coro_call = builder.ins().call(is_coroutine_local, &[*func_bits]);
                    let is_coro_bits = builder.inst_results(is_coro_call)[0];
                    let is_coro_truthy_call = builder.ins().call(truthy_local, &[is_coro_bits]);
                    let is_coro_truthy_bits = builder.inst_results(is_coro_truthy_call)[0];
                    let is_coro_bool =
                        builder
                            .ins()
                            .icmp_imm(IntCC::NotEqual, is_coro_truthy_bits, 0);
                    let func_direct_block = builder.create_block();
                    let func_closure_block = builder.create_block();
                    let func_non_gen_block = builder.create_block();
                    let func_non_special_block = builder.create_block();
                    brif_block(
                        &mut builder,
                        is_gen_bool,
                        func_closure_block,
                        &[],
                        func_non_gen_block,
                        &[],
                    );

                    builder.switch_to_block(func_non_gen_block);
                    builder.seal_block(func_non_gen_block);
                    brif_block(
                        &mut builder,
                        is_coro_bool,
                        func_closure_block,
                        &[],
                        func_non_special_block,
                        &[],
                    );

                    builder.switch_to_block(func_non_special_block);
                    builder.seal_block(func_non_special_block);
                    brif_block(
                        &mut builder,
                        closure_is_zero,
                        func_direct_block,
                        &[],
                        func_closure_block,
                        &[],
                    );

                    builder.switch_to_block(func_closure_block);
                    builder.seal_block(func_closure_block);
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
                    let closure_label = format!("{call_site_prefix}_closure");
                    let site_bits = builder.ins().iconst(
                        types::I64,
                        box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            closure_label.as_str(),
                        )),
                    );
                    let closure_call = builder
                        .ins()
                        .call(call_bind_local, &[site_bits, *func_bits, callargs_ptr]);
                    let closure_res = builder.inst_results(closure_call)[0];
                    jump_block(&mut builder, merge_block, &[closure_res]);

                    builder.switch_to_block(func_direct_block);
                    builder.seal_block(func_direct_block);
                    let resolve_call = builder.ins().call(resolve_local, &[*func_bits]);
                    let func_ptr = builder.inst_results(resolve_call)[0];
                    let func_arity =
                        builder
                            .ins()
                            .load(types::I64, MemFlags::trusted(), func_ptr, 8);
                    let provided_arity = builder.ins().iconst(types::I64, args.len() as i64);
                    let arity_match = builder.ins().icmp(IntCC::Equal, func_arity, provided_arity);
                    let func_direct_call_block = builder.create_block();
                    let func_bind_block = builder.create_block();
                    brif_block(
                        &mut builder,
                        arity_match,
                        func_direct_call_block,
                        &[],
                        func_bind_block,
                        &[],
                    );

                    builder.switch_to_block(func_bind_block);
                    builder.seal_block(func_bind_block);
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
                    let bind_label = format!("{call_site_prefix}_bind");
                    let site_bits = builder.ins().iconst(
                        types::I64,
                        box_int(stable_ic_site_id(
                            func_ir.name.as_str(),
                            op_idx,
                            bind_label.as_str(),
                        )),
                    );
                    let bind_call = builder
                        .ins()
                        .call(call_bind_local, &[site_bits, *func_bits, callargs_ptr]);
                    let bind_res = builder.inst_results(bind_call)[0];
                    jump_block(&mut builder, merge_block, &[bind_res]);

                    builder.switch_to_block(func_direct_call_block);
                    builder.seal_block(func_direct_call_block);
                    let fn_ptr = builder
                        .ins()
                        .load(types::I64, MemFlags::trusted(), func_ptr, 0);

                    let mut sig = self.module.make_signature();
                    for _ in 0..args.len() {
                        sig.params.push(AbiParam::new(types::I64));
                    }
                    sig.returns.push(AbiParam::new(types::I64));
                    let sig_ref = builder.import_signature(sig);
                    let guard_call = builder.ins().call(guard_enter_local, &[]);
                    let guard_val = builder.inst_results(guard_call)[0];
                    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
                    let func_call_block = builder.create_block();
                    let func_fail_block = builder.create_block();
                    builder
                        .ins()
                        .brif(guard_ok, func_call_block, &[], func_fail_block, &[]);

                    builder.switch_to_block(func_call_block);
                    builder.seal_block(func_call_block);
                    if emit_traces {
                        let _ = builder.ins().call(trace_enter_local, &[*func_bits]);
                    }
                    let call = builder.ins().call_indirect(sig_ref, fn_ptr, &args);
                    let res = builder.inst_results(call)[0];
                    if emit_traces {
                        let _ = builder.ins().call(trace_exit_local, &[]);
                    }
                    let _ = builder.ins().call(guard_exit_local, &[]);
                    jump_block(&mut builder, merge_block, &[res]);

                    builder.switch_to_block(func_fail_block);
                    builder.seal_block(func_fail_block);
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    jump_block(&mut builder, merge_block, &[none_bits]);

                    builder.switch_to_block(merge_block);
                    builder.seal_block(merge_block);
                    let res = builder.block_params(merge_block)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "invoke_ffi" => {
                    let args_names = op.args.as_ref().unwrap();
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

                    let mut invoke_sig = self.module.make_signature();
                    invoke_sig.params.push(AbiParam::new(types::I64));
                    invoke_sig.params.push(AbiParam::new(types::I64));
                    invoke_sig.params.push(AbiParam::new(types::I64));
                    invoke_sig.params.push(AbiParam::new(types::I64));
                    invoke_sig.returns.push(AbiParam::new(types::I64));
                    let invoke_fn = self
                        .module
                        .declare_function("molt_invoke_ffi_ic", Linkage::Import, &invoke_sig)
                        .unwrap();
                    let invoke_local = self.module.declare_func_in_func(invoke_fn, builder.func);
                    let invoke_call = builder.ins().call(
                        invoke_local,
                        &[site_bits, *func_bits, callargs_ptr, require_bridge_cap],
                    );
                    let res = builder.inst_results(invoke_call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "call_bind" | "call_indirect" => {
                    let args_names = op.args.as_ref().unwrap();
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);

                    // `molt_call_bind*` consumes the CallArgs builder pointer and decrefs it
                    // internally (see `PtrDropGuard` in runtime). The backend's lifetime tracking
                    // must therefore *not* emit an additional decref for the builder variable,
                    // or we'll double-free the CallArgs object and corrupt unrelated state.
                    //
                    // This is a semantic ownership transfer: the builder must not be used after
                    // the call_bind op. If IR ever violates this, it is a compiler bug.
                    let callargs_name = &args_names[1];
                    let last = last_use.get(callargs_name).copied().unwrap_or(op_idx);
                    if last > op_idx {
                        panic!(
                            "call_bind consumes callargs builder {}, but it is used later (func={} op_idx={} last_use={})",
                            callargs_name, func_ir.name, op_idx, last
                        );
                    }
                    if let Some(block) = builder.current_block() {
                        if block == entry_block && loop_depth == 0 {
                            tracked_obj_vars.retain(|n| n != callargs_name);
                            tracked_vars.retain(|n| n != callargs_name);
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
                    let args_names = op.args.as_ref().unwrap();
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
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "module_new" => {
                    let args = op.args.as_ref().unwrap();
                    let name_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*name_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "class_new" => {
                    let args = op.args.as_ref().unwrap();
                    let name_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_class_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*name_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "builtin_type" => {
                    let args = op.args.as_ref().unwrap();
                    let tag_bits = var_get(&mut builder, &vars, &args[0]).expect("Tag not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_builtin_type", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*tag_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "type_of" => {
                    let args = op.args.as_ref().unwrap();
                    let obj_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_type_of", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "is_native_awaitable" => {
                    let args = op.args.as_ref().unwrap();
                    let obj_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is_native_awaitable", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "class_layout_version" => {
                    let args = op.args.as_ref().unwrap();
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_class_layout_version", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*class_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "class_set_layout_version" => {
                    let args = op.args.as_ref().unwrap();
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let version_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Version not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_class_set_layout_version", Linkage::Import, &sig)
                        .unwrap();
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
                    let args = op.args.as_ref().unwrap();
                    let obj_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_isinstance", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj_bits, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "issubclass" => {
                    let args = op.args.as_ref().unwrap();
                    let sub_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Subclass not found");
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_issubclass", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*sub_bits, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "object_new" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_object_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "class_set_base" => {
                    let args = op.args.as_ref().unwrap();
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let base_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Base class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_class_set_base", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*class_bits, *base_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "class_apply_set_name" => {
                    let args = op.args.as_ref().unwrap();
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_class_apply_set_name", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*class_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "super_new" => {
                    let args = op.args.as_ref().unwrap();
                    let type_bits = var_get(&mut builder, &vars, &args[0]).expect("Type not found");
                    let obj_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Object not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_super_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*type_bits, *obj_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "classmethod_new" => {
                    let args = op.args.as_ref().unwrap();
                    let func_bits = var_get(&mut builder, &vars, &args[0]).expect("Func not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_classmethod_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*func_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "staticmethod_new" => {
                    let args = op.args.as_ref().unwrap();
                    let func_bits = var_get(&mut builder, &vars, &args[0]).expect("Func not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_staticmethod_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*func_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "property_new" => {
                    let args = op.args.as_ref().unwrap();
                    let getter = var_get(&mut builder, &vars, &args[0]).expect("Getter not found");
                    let setter = var_get(&mut builder, &vars, &args[1]).expect("Setter not found");
                    let deleter =
                        var_get(&mut builder, &vars, &args[2]).expect("Deleter not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_property_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*getter, *setter, *deleter]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "object_set_class" => {
                    let args = op.args.as_ref().unwrap();
                    let obj_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj_bits);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_object_set_class", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[obj_ptr, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "module_cache_get" => {
                    let args = op.args.as_ref().unwrap();
                    let name_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_cache_get", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*name_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "module_import" => {
                    let args = op.args.as_ref().unwrap();
                    let name_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_import", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*name_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "module_cache_set" => {
                    let args = op.args.as_ref().unwrap();
                    let name_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module name not found");
                    let module_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Module not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_cache_set", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder
                        .ins()
                        .call(local_callee, &[*name_bits, *module_bits]);
                }
                "module_cache_del" => {
                    let args = op.args.as_ref().unwrap();
                    let name_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_cache_del", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*name_bits]);
                }
                "module_get_attr" => {
                    let args = op.args.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_get_attr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*module_bits, *attr_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "module_get_global" => {
                    let args = op.args.as_ref().unwrap();
                    let module_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module not found");
                    let attr_bits = var_get(&mut builder, &vars, &args[1]).expect("Attr not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_get_global", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*module_bits, *attr_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "module_del_global" => {
                    let args = op.args.as_ref().unwrap();
                    let module_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module not found");
                    let attr_bits = var_get(&mut builder, &vars, &args[1]).expect("Attr not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_del_global", Linkage::Import, &sig)
                        .unwrap();
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
                    let args = op.args.as_ref().unwrap();
                    let module_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module not found");
                    let attr_bits = var_get(&mut builder, &vars, &args[1]).expect("Attr not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_get_name", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*module_bits, *attr_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "module_set_attr" => {
                    let args = op.args.as_ref().unwrap();
                    let module_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module not found");
                    let attr_bits = var_get(&mut builder, &vars, &args[1]).expect("Attr not found");
                    let val_bits = var_get(&mut builder, &vars, &args[2]).unwrap_or_else(|| {
                        panic!(
                            "Value not found for module_set_attr in {} op {}",
                            func_ir.name, op_idx
                        )
                    });
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_set_attr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder
                        .ins()
                        .call(local_callee, &[*module_bits, *attr_bits, *val_bits]);
                }
                "module_import_star" => {
                    let args = op.args.as_ref().unwrap();
                    let src_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Module not found");
                    let dst_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Module not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_module_import_star", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*src_bits, *dst_bits]);
                }
                "context_null" => {
                    let args = op.args.as_ref().unwrap();
                    let payload =
                        var_get(&mut builder, &vars, &args[0]).expect("Payload not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_null", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*payload]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "context_enter" => {
                    let args = op.args.as_ref().unwrap();
                    let ctx = var_get(&mut builder, &vars, &args[0]).expect("Context not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_enter", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*ctx]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "context_exit" => {
                    let args = op.args.as_ref().unwrap();
                    let ctx = var_get(&mut builder, &vars, &args[0]).expect("Context not found");
                    let exc = var_get(&mut builder, &vars, &args[1]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_exit", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*ctx, *exc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "context_closing" => {
                    let args = op.args.as_ref().unwrap();
                    let payload =
                        var_get(&mut builder, &vars, &args[0]).expect("Payload not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_closing", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*payload]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "context_unwind" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_unwind", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "context_depth" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_depth", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "context_unwind_to" => {
                    let args = op.args.as_ref().unwrap();
                    let depth = var_get(&mut builder, &vars, &args[0]).expect("Depth not found");
                    let exc = var_get(&mut builder, &vars, &args[1]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_context_unwind_to", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*depth, *exc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_push" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_push", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_pop" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_pop", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_stack_clear" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_stack_clear", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_stack_depth" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_stack_depth", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_stack_enter" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_stack_enter", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_stack_exit" => {
                    let args = op.args.as_ref().unwrap();
                    let prev = var_get(&mut builder, &vars, &args[0])
                        .expect("exception baseline not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_stack_exit", Linkage::Import, &sig)
                        .unwrap();
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
                    let args = op.args.as_ref().unwrap();
                    let depth =
                        var_get(&mut builder, &vars, &args[0]).expect("exception depth not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_stack_set_depth", Linkage::Import, &sig)
                        .unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_last", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "getargv" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_getargv", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "getframe" => {
                    let args = op.args.as_ref().unwrap();
                    let depth = var_get(&mut builder, &vars, &args[0]).expect("depth not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_getframe", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*depth]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "sys_executable" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_sys_executable", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_new" => {
                    let args = op.args.as_ref().unwrap();
                    let kind = var_get(&mut builder, &vars, &args[0]).expect("Kind not found");
                    let args_bits = var_get(&mut builder, &vars, &args[1]).expect("Args not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_new", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*kind, *args_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_new_from_class" => {
                    let args = op.args.as_ref().unwrap();
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let args_bits = var_get(&mut builder, &vars, &args[1]).expect("Args not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_new_from_class", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*class_bits, *args_bits]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exceptiongroup_match" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let matcher =
                        var_get(&mut builder, &vars, &args[1]).expect("Matcher not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exceptiongroup_match", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc, *matcher]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exceptiongroup_combine" => {
                    let args = op.args.as_ref().unwrap();
                    let items =
                        var_get(&mut builder, &vars, &args[0]).expect("Exception list not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exceptiongroup_combine", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*items]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_clear" => {
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_clear", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_kind" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_kind", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_class" => {
                    let args = op.args.as_ref().unwrap();
                    let kind =
                        var_get(&mut builder, &vars, &args[0]).expect("Exception kind not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_class", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*kind]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_message" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_message", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_set_cause" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let cause = var_get(&mut builder, &vars, &args[1]).expect("Cause not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_set_cause", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc, *cause]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_set_last" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_set_last", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_set_value" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let value = var_get(&mut builder, &vars, &args[1]).expect("Value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_set_value", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc, *value]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "exception_context_set" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_context_set", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*exc]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "raise" => {
                    let args = op.args.as_ref().unwrap();
                    let exc = var_get(&mut builder, &vars, &args[0]).expect("Exception not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_raise", Linkage::Import, &sig)
                        .unwrap();
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
                    let target_id = op.value.unwrap();
                    let target_block = state_blocks[&target_id];
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
                        }
                        if std::env::var("MOLT_DEBUG_CHECK_EXCEPTION").as_deref() == Ok("1")
                            && func_ir.name.contains("_tmp_compress_repro11b__f")
                        {
                            eprintln!("check_exception {} op={}", func_ir.name, op_idx,);
                        }
                    }
                    if !carry_obj.is_empty() {
                        let cleanup =
                            drain_cleanup_tracked(&mut carry_obj, &last_use, op_idx, None);
                        for name in cleanup {
                            let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                panic!(
                                    "Tracked obj var not found in {} op {}: {}",
                                    func_ir.name, op_idx, name
                                )
                            });
                            builder.ins().call(local_dec_ref_obj, &[*val]);
                        }
                    }
                    if !carry_ptr.is_empty() {
                        let cleanup =
                            drain_cleanup_tracked(&mut carry_ptr, &last_use, op_idx, None);
                        for name in cleanup {
                            let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                panic!(
                                    "Tracked ptr var not found in {} op {}: {}",
                                    func_ir.name, op_idx, name
                                )
                            });
                            builder.ins().call(local_dec_ref, &[*val]);
                        }
                    }
                    let mut sig = self.module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_exception_pending_fast", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[]);
                    let pending = builder.inst_results(call)[0];
                    let cond = builder.ins().icmp_imm(IntCC::NotEqual, pending, 0);
                    let fallthrough = builder.create_block();
                    reachable_blocks.insert(target_block);
                    reachable_blocks.insert(fallthrough);
                    brif_block(&mut builder, cond, target_block, &[], fallthrough, &[]);
                    switch_to_block_tracking(&mut builder, fallthrough, &mut is_block_filled);
                    if !carry_obj.is_empty() {
                        block_tracked_obj
                            .entry(fallthrough)
                            .or_default()
                            .extend(carry_obj);
                    }
                    if !carry_ptr.is_empty() {
                        block_tracked_ptr
                            .entry(fallthrough)
                            .or_default()
                            .extend(carry_ptr);
                    }
                }
                "file_open" => {
                    let args = op.args.as_ref().unwrap();
                    let path = var_get(&mut builder, &vars, &args[0]).expect("Path not found");
                    let mode = var_get(&mut builder, &vars, &args[1]).expect("Mode not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_file_open", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*path, *mode]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "file_read" => {
                    let args = op.args.as_ref().unwrap();
                    let handle = var_get(&mut builder, &vars, &args[0]).expect("Handle not found");
                    let size = var_get(&mut builder, &vars, &args[1]).expect("Size not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_file_read", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*handle, *size]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "file_write" => {
                    let args = op.args.as_ref().unwrap();
                    let handle = var_get(&mut builder, &vars, &args[0]).expect("Handle not found");
                    let data = var_get(&mut builder, &vars, &args[1]).expect("Data not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_file_write", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*handle, *data]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "file_close" => {
                    let args = op.args.as_ref().unwrap();
                    let handle = var_get(&mut builder, &vars, &args[0]).expect("Handle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_file_close", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*handle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "file_flush" => {
                    let args = op.args.as_ref().unwrap();
                    let handle = var_get(&mut builder, &vars, &args[0]).expect("Handle not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_file_flush", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*handle]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "bridge_unavailable" => {
                    let args = op.args.as_ref().unwrap();
                    let msg = var_get(&mut builder, &vars, &args[0]).expect("Message not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_bridge_unavailable", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*msg]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "if" => {
                    let args = op.args.as_ref().unwrap();
                    let cond = var_get(&mut builder, &vars, &args[0]).expect("Cond not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is_truthy", Linkage::Import, &sig)
                        .unwrap();
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
                        builder.ins().call(local_dec_ref, &[*val]);
                    }
                    let has_explicit_else = if_to_else.contains_key(&op_idx);
                    let end_if_idx = *if_to_end_if
                        .get(&op_idx)
                        .expect("if without matching end_if");
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
                                builder.ins().call(local_dec_ref, &[*val]);
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
                                    builder.ins().call(local_dec_ref, &[*val]);
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
                                    builder.ins().call(local_dec_ref, &[*val]);
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
                                    builder.ins().call(local_dec_ref, &[*val]);
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
                        if !frame.phi_ops.is_empty() {
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
                                let mut remove_names: HashSet<&str> =
                                    HashSet::with_capacity(frame.phi_ops.len() * 2);
                                for (_out, then_name, else_name) in &frame.phi_ops {
                                    remove_names.insert(then_name.as_str());
                                    remove_names.insert(else_name.as_str());
                                }
                                tracked.retain(|name| !remove_names.contains(name.as_str()));
                                let mut present: HashSet<String> =
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
                    let args = op.args.as_ref().unwrap();
                    let start =
                        var_get(&mut builder, &vars, &args[0]).expect("Loop index start not found");
                    let out_name = op.out.unwrap();
                    let loop_block = builder.create_block();
                    let body_block = builder.create_block();
                    let after_block = builder.create_block();
                    let idx_param = builder.append_block_param(loop_block, types::I64);
                    if !is_block_filled {
                        ensure_block_in_layout(&mut builder, loop_block);
                        reachable_blocks.insert(loop_block);
                        jump_block(&mut builder, loop_block, &[*start]);
                        switch_to_block_tracking(&mut builder, loop_block, &mut is_block_filled);
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
                    let args = op.args.as_ref().unwrap();
                    let cond =
                        var_get(&mut builder, &vars, &args[0]).expect("Loop break cond not found");
                    let frame = loop_stack.last().expect("No loop on stack");
                    let current_block = builder
                        .current_block()
                        .expect("loop_break_if_true requires an active block");
                    let tracked_obj_snapshot = block_tracked_obj
                        .get(&current_block)
                        .map(|names| collect_cleanup_tracked(names, &last_use, op_idx, None))
                        .unwrap_or_default();
                    let tracked_ptr_snapshot = block_tracked_ptr
                        .get(&current_block)
                        .map(|names| collect_cleanup_tracked(names, &last_use, op_idx, None))
                        .unwrap_or_default();
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is_truthy", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*cond]);
                    let truthy = builder.inst_results(call)[0];
                    let cond_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0);
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
                        builder.ins().call(local_dec_ref, &[*val]);
                    }
                    reachable_blocks.insert(frame.after_block);
                    jump_block(&mut builder, frame.after_block, &[]);
                    switch_to_block_tracking(&mut builder, frame.body_block, &mut is_block_filled);
                }
                "loop_break_if_false" => {
                    let args = op.args.as_ref().unwrap();
                    let cond =
                        var_get(&mut builder, &vars, &args[0]).expect("Loop break cond not found");
                    let frame = loop_stack.last().expect("No loop on stack");
                    let current_block = builder
                        .current_block()
                        .expect("loop_break_if_false requires an active block");
                    let tracked_obj_snapshot = block_tracked_obj
                        .get(&current_block)
                        .map(|names| collect_cleanup_tracked(names, &last_use, op_idx, None))
                        .unwrap_or_default();
                    let tracked_ptr_snapshot = block_tracked_ptr
                        .get(&current_block)
                        .map(|names| collect_cleanup_tracked(names, &last_use, op_idx, None))
                        .unwrap_or_default();
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_is_truthy", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*cond]);
                    let truthy = builder.inst_results(call)[0];
                    let cond_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0);
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
                        builder.ins().call(local_dec_ref, &[*val]);
                    }
                    reachable_blocks.insert(frame.after_block);
                    jump_block(&mut builder, frame.after_block, &[]);
                    switch_to_block_tracking(&mut builder, frame.body_block, &mut is_block_filled);
                }
                "loop_break" => {
                    let frame = loop_stack.last().unwrap_or_else(|| {
                        panic!("No loop on stack in {} at op {}", func_ir.name, op_idx)
                    });
                    let current_block = builder
                        .current_block()
                        .expect("loop_break requires an active block");
                    if let Some(names) = block_tracked_obj.get_mut(&current_block) {
                        let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                        for name in cleanup {
                            let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                panic!(
                                    "Tracked obj var not found in {} op {}: {}",
                                    func_ir.name, op_idx, name
                                )
                            });
                            builder.ins().call(local_dec_ref_obj, &[*val]);
                        }
                    }
                    if let Some(names) = block_tracked_ptr.get_mut(&current_block) {
                        let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                        for name in cleanup {
                            let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                panic!(
                                    "Tracked ptr var not found in {} op {}: {}",
                                    func_ir.name, op_idx, name
                                )
                            });
                            builder.ins().call(local_dec_ref, &[*val]);
                        }
                    }
                    reachable_blocks.insert(frame.after_block);
                    jump_block(&mut builder, frame.after_block, &[]);
                    is_block_filled = true;
                }
                "loop_index_next" => {
                    let args = op.args.as_ref().unwrap();
                    let next_idx =
                        var_get(&mut builder, &vars, &args[0]).expect("Loop index next not found");
                    let frame = loop_stack.last_mut().unwrap_or_else(|| {
                        panic!("No loop on stack in {} at op {}", func_ir.name, op_idx)
                    });
                    frame.next_index = Some(*next_idx);
                    let out_name = op.out.unwrap();
                    def_var_named(&mut builder, &vars, out_name, *next_idx);
                }
                "loop_continue" => {
                    let frame = loop_stack.last_mut().unwrap_or_else(|| {
                        panic!("No loop on stack in {} at op {}", func_ir.name, op_idx)
                    });
                    let current_block = builder
                        .current_block()
                        .expect("loop_continue requires an active block");
                    if let Some(names) = block_tracked_obj.get_mut(&current_block) {
                        let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                        for name in cleanup {
                            let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                panic!(
                                    "Tracked obj var not found in {} op {}: {}",
                                    func_ir.name, op_idx, name
                                )
                            });
                            builder.ins().call(local_dec_ref_obj, &[*val]);
                        }
                    }
                    if let Some(names) = block_tracked_ptr.get_mut(&current_block) {
                        let cleanup = drain_cleanup_tracked(names, &last_use, op_idx, None);
                        for name in cleanup {
                            let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                panic!(
                                    "Tracked ptr var not found in {} op {}: {}",
                                    func_ir.name, op_idx, name
                                )
                            });
                            builder.ins().call(local_dec_ref, &[*val]);
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
                "loop_end" => {
                    let mut frame = loop_stack.pop().unwrap_or_else(|| {
                        panic!("No loop on stack in {} at op {}", func_ir.name, op_idx)
                    });
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
                "alloc" => {
                    let size = op.value.unwrap();
                    let iconst = builder.ins().iconst(types::I64, size);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64)); // Returns object bits
                    let callee = self
                        .module
                        .declare_function("molt_alloc", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst]);
                    let res = builder.inst_results(call)[0];
                    let out_name = op.out.unwrap();
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "alloc_class" => {
                    let size = op.value.unwrap();
                    let args = op.args.as_ref().unwrap();
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let iconst = builder.ins().iconst(types::I64, size);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64)); // Returns object bits
                    let callee = self
                        .module
                        .declare_function("molt_alloc_class", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    let out_name = op.out.unwrap();
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "alloc_class_trusted" => {
                    let size = op.value.unwrap();
                    let args = op.args.as_ref().unwrap();
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let iconst = builder.ins().iconst(types::I64, size);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64)); // Returns object bits
                    let callee = self
                        .module
                        .declare_function("molt_alloc_class_trusted", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    let out_name = op.out.unwrap();
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "alloc_class_static" => {
                    let size = op.value.unwrap();
                    let args = op.args.as_ref().unwrap();
                    let class_bits =
                        var_get(&mut builder, &vars, &args[0]).expect("Class not found");
                    let iconst = builder.ins().iconst(types::I64, size);

                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64)); // Returns object bits
                    let callee = self
                        .module
                        .declare_function("molt_alloc_class_static", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[iconst, *class_bits]);
                    let res = builder.inst_results(call)[0];
                    let out_name = op.out.unwrap();
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "alloc_task" => {
                    let closure_size = op.value.unwrap();
                    let task_kind = op.task_kind.as_deref().unwrap_or("future");
                    let (kind_bits, payload_base) = match task_kind {
                        "generator" => (TASK_KIND_GENERATOR, GENERATOR_CONTROL_BYTES),
                        "future" => (TASK_KIND_FUTURE, 0),
                        "coroutine" => (TASK_KIND_COROUTINE, 0),
                        _ => panic!("unknown task kind: {task_kind}"),
                    };
                    let size = builder.ins().iconst(types::I64, closure_size);

                    let poll_func_name = op.s_value.as_ref().unwrap();
                    let mut poll_sig = self.module.make_signature();
                    poll_sig.params.push(AbiParam::new(types::I64));
                    poll_sig.returns.push(AbiParam::new(types::I64));

                    let poll_func_id = self
                        .module
                        .declare_function(poll_func_name, Linkage::Export, &poll_sig)
                        .unwrap();
                    let poll_func_ref =
                        self.module.declare_func_in_func(poll_func_id, builder.func);
                    let poll_addr = builder.ins().func_addr(types::I64, poll_func_ref);

                    let mut task_sig = self.module.make_signature();
                    task_sig.params.push(AbiParam::new(types::I64));
                    task_sig.params.push(AbiParam::new(types::I64));
                    task_sig.params.push(AbiParam::new(types::I64));
                    task_sig.returns.push(AbiParam::new(types::I64));
                    let task_callee = self
                        .module
                        .declare_function("molt_task_new", Linkage::Import, &task_sig)
                        .unwrap();
                    let task_local = self.module.declare_func_in_func(task_callee, builder.func);
                    let kind_val = builder.ins().iconst(types::I64, kind_bits);
                    let call = builder.ins().call(task_local, &[poll_addr, size, kind_val]);
                    let obj = builder.inst_results(call)[0];
                    let obj_ptr = unbox_ptr_value(&mut builder, obj);
                    if let Some(args_names) = &op.args {
                        for (i, name) in args_names.iter().enumerate() {
                            let arg_val = var_get(&mut builder, &vars, name)
                                .expect("Arg not found for alloc_task");
                            let offset = payload_base + (i * 8) as i32;
                            builder
                                .ins()
                                .store(MemFlags::trusted(), *arg_val, obj_ptr, offset);
                            emit_maybe_ref_adjust_v2(&mut builder, *arg_val, local_inc_ref_obj);
                        }
                    }
                    if matches!(task_kind, "future" | "coroutine") {
                        let mut get_sig = self.module.make_signature();
                        get_sig.returns.push(AbiParam::new(types::I64));
                        let get_callee = self
                            .module
                            .declare_function(
                                "molt_cancel_token_get_current",
                                Linkage::Import,
                                &get_sig,
                            )
                            .unwrap();
                        let get_local = self.module.declare_func_in_func(get_callee, builder.func);
                        let get_call = builder.ins().call(get_local, &[]);
                        let current_token = builder.inst_results(get_call)[0];

                        let mut reg_sig = self.module.make_signature();
                        reg_sig.params.push(AbiParam::new(types::I64));
                        reg_sig.params.push(AbiParam::new(types::I64));
                        reg_sig.returns.push(AbiParam::new(types::I64));
                        let reg_callee = self
                            .module
                            .declare_function(
                                "molt_task_register_token_owned",
                                Linkage::Import,
                                &reg_sig,
                            )
                            .unwrap();
                        let reg_local = self.module.declare_func_in_func(reg_callee, builder.func);
                        builder.ins().call(reg_local, &[obj, current_token]);
                    }

                    output_is_ptr = false;
                    let out_name = op.out.unwrap();
                    def_var_named(&mut builder, &vars, out_name, obj);
                }
                "store" => {
                    let local_profile_struct =
                        local_profile_struct.expect("store lowering requires profile import");
                    let profile_enabled_val =
                        profile_enabled_val.expect("store lowering requires profile flag");
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Value not found");
                    let offset = op.value.unwrap() as i32;
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_object_field_set_ptr", Linkage::Import, &sig)
                        .unwrap();
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
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Value not found");
                    let offset = op.value.unwrap() as i32;
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let offset_bits = builder.ins().iconst(types::I64, i64::from(offset));
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_object_field_init_ptr", Linkage::Import, &sig)
                        .unwrap();
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
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let offset = op.value.unwrap() as i32;
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let res = builder
                        .ins()
                        .load(types::I64, MemFlags::trusted(), obj_ptr, offset);
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj);
                    let out_name = op.out.unwrap();
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "closure_load" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap());
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_closure_load", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[obj_ptr, offset]);
                    let res = builder.inst_results(call)[0];
                    let out_name = op.out.unwrap();
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "closure_store" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Value not found");
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap());
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_closure_store", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[obj_ptr, offset, *val]);
                    if let Some(out_name) = op.out {
                        let res = builder.inst_results(call)[0];
                        def_var_named(&mut builder, &vars, out_name, res);
                    }
                }
                "guarded_load" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let offset = op.value.unwrap() as i32;
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let res = builder
                        .ins()
                        .load(types::I64, MemFlags::trusted(), obj_ptr, offset);
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj);
                    let out_name = op.out.unwrap();
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "guarded_field_get" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let expected_version =
                        var_get(&mut builder, &vars, &args[2]).expect("Expected version not found");
                    let attr_name = op.s_value.as_ref().unwrap();
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
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap());
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_guarded_field_get_ptr", Linkage::Import, &sig)
                        .unwrap();
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
                    let out_name = op.out.unwrap();
                    def_var_named(&mut builder, &vars, out_name, res);
                }
                "guarded_field_set" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let expected_version =
                        var_get(&mut builder, &vars, &args[2]).expect("Expected version not found");
                    let val = var_get(&mut builder, &vars, &args[3]).expect("Value not found");
                    let attr_name = op.s_value.as_ref().unwrap();
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
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap());
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_guarded_field_set_ptr", Linkage::Import, &sig)
                        .unwrap();
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
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).expect("Object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Class not found");
                    let expected_version =
                        var_get(&mut builder, &vars, &args[2]).expect("Expected version not found");
                    let val = var_get(&mut builder, &vars, &args[3]).expect("Value not found");
                    let attr_name = op.s_value.as_ref().unwrap();
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
                    let offset = builder.ins().iconst(types::I64, op.value.unwrap());
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_guarded_field_init_ptr", Linkage::Import, &sig)
                        .unwrap();
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
                    let args = op.args.as_ref().unwrap();
                    let val =
                        var_get(&mut builder, &vars, &args[0]).expect("Guard value not found");
                    let expected = var_get(&mut builder, &vars, &args[1])
                        .expect("Guard expected tag not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_guard_type", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    builder.ins().call(local_callee, &[*val, *expected]);
                }
                "guard_layout" | "guard_dict_shape" => {
                    let args = op.args.as_ref().unwrap();
                    let obj =
                        var_get(&mut builder, &vars, &args[0]).expect("Guard object not found");
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let class_bits =
                        var_get(&mut builder, &vars, &args[1]).expect("Guard class not found");
                    let expected_version =
                        var_get(&mut builder, &vars, &args[2]).expect("Guard version not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_guard_layout_ptr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[obj_ptr, *class_bits, *expected_version]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "get_attr_generic_ptr" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let attr_name = op.s_value.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_get_attr_ptr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[obj_ptr, attr_ptr, attr_len]);
                    let res = builder.inst_results(call)[0];
                    // Attribute lookup may return borrowed values from object/class internals.
                    // Normalize to an owned reference so last-use decref remains safe.
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj);
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "get_attr_generic_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let attr_name = op.s_value.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_get_attr_object_ic", Linkage::Import, &sig)
                        .unwrap();
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
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj);
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "get_attr_special_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let attr_name = op.s_value.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_get_attr_special", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*obj, attr_ptr, attr_len]);
                    let res = builder.inst_results(call)[0];
                    // Keep attribute result ownership consistent across all get-attr ops.
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj);
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "get_attr_name" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = var_get(&mut builder, &vars, &args[1]).expect("Attr name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_get_attr_name", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name]);
                    let res = builder.inst_results(call)[0];
                    // Attribute lookup returns a borrowed reference from object internals/dicts in
                    // some fast paths. Convert it to an owned reference so lifetime tracking can
                    // safely decref at last use without corrupting dict-owned values.
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj);
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "get_attr_name_default" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = var_get(&mut builder, &vars, &args[1]).expect("Attr name not found");
                    let default =
                        var_get(&mut builder, &vars, &args[2]).expect("Attr default not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_get_attr_name_default", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name, *default]);
                    let res = builder.inst_results(call)[0];
                    // See `get_attr_name` above: ensure the returned value is owned.
                    emit_maybe_ref_adjust_v2(&mut builder, res, local_inc_ref_obj);
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "has_attr_name" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = var_get(&mut builder, &vars, &args[1]).expect("Attr name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_has_attr_name", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "set_attr_name" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = var_get(&mut builder, &vars, &args[1]).expect("Attr name not found");
                    let val = var_get(&mut builder, &vars, &args[2]).expect("Attr value not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_attr_name", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "set_attr_generic_ptr" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Attr value not found");
                    let attr_name = op.s_value.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_attr_ptr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[obj_ptr, attr_ptr, attr_len, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "set_attr_generic_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let val = var_get(&mut builder, &vars, &args[1]).expect("Attr value not found");
                    let attr_name = op.s_value.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_set_attr_object", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*obj, attr_ptr, attr_len, *val]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "del_attr_generic_ptr" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let obj_ptr = unbox_ptr_value(&mut builder, *obj);
                    let attr_name = op.s_value.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_del_attr_ptr", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[obj_ptr, attr_ptr, attr_len]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "del_attr_generic_obj" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let attr_name = op.s_value.as_ref().unwrap();
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
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_del_attr_object", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*obj, attr_ptr, attr_len]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "del_attr_name" => {
                    let args = op.args.as_ref().unwrap();
                    let obj = var_get(&mut builder, &vars, &args[0]).unwrap_or_else(|| {
                        panic!("Attr object not found in {} op {}", func_ir.name, op_idx)
                    });
                    let name = var_get(&mut builder, &vars, &args[1]).expect("Attr name not found");
                    let mut sig = self.module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let callee = self
                        .module
                        .declare_function("molt_del_attr_name", Linkage::Import, &sig)
                        .unwrap();
                    let local_callee = self.module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *name]);
                    let res = builder.inst_results(call)[0];
                    def_var_named(&mut builder, &vars, op.out.unwrap(), res);
                }
                "ret" => {
                    if std::env::var("MOLT_DEBUG_RET_CLEANUP").as_deref() == Ok("1")
                        && (func_ir
                            .name
                            .contains("open0_dead_comp_capture_probe__touch")
                            || func_ir.name == "__main____touch")
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
                                    builder.ins().call(local_dec_ref, &[*val]);
                                }
                            }
                        }
                        for name in &tracked_vars {
                            if let Some(val) = entry_vars.get(name) {
                                builder.ins().call(local_dec_ref, &[*val]);
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
                                if name == *var_name {
                                    continue;
                                }
                                let val =
                                    var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                        panic!(
                                            "Tracked ptr var not found in {} op {}: {}",
                                            func_ir.name, op_idx, name
                                        )
                                    });
                                builder.ins().call(local_dec_ref, &[*val]);
                            }
                        }
                    }
                    tracked_vars.retain(|v| v != var_name);
                    tracked_obj_vars.retain(|v| v != var_name);
                    for name in &tracked_vars {
                        if let Some(val) = entry_vars.get(name) {
                            builder.ins().call(local_dec_ref, &[*val]);
                        }
                    }
                    for name in &tracked_obj_vars {
                        if let Some(val) = entry_vars.get(name) {
                            builder.ins().call(local_dec_ref_obj, &[*val]);
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
                                builder.ins().call(local_dec_ref, &[*val]);
                            }
                        }
                    }
                    for name in &tracked_vars {
                        if let Some(val) = entry_vars.get(name) {
                            builder.ins().call(local_dec_ref, &[*val]);
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
                    let target_id = op.value.unwrap();
                    let target_block = state_blocks[&target_id];
                    if let Some(block) = builder.current_block() {
                        let mut carry_obj = block_tracked_obj.remove(&block).unwrap_or_default();
                        let cleanup =
                            drain_cleanup_tracked(&mut carry_obj, &last_use, op_idx, None);
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
                                carry_obj,
                            );
                        }

                        let mut carry_ptr = block_tracked_ptr.remove(&block).unwrap_or_default();
                        let cleanup =
                            drain_cleanup_tracked(&mut carry_ptr, &last_use, op_idx, None);
                        for name in cleanup {
                            let val = var_get(&mut builder, &vars, &name).unwrap_or_else(|| {
                                panic!(
                                    "Tracked ptr var not found in {} op {}: {}",
                                    func_ir.name, op_idx, name
                                )
                            });
                            builder.ins().call(local_dec_ref, &[*val]);
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
                    let args = op.args.as_ref().unwrap();
                    let cond = var_get(&mut builder, &vars, &args[0]).expect("Cond not found");
                    let target_id = op.value.unwrap();
                    let target_block = state_blocks[&target_id];
                    let origin_block = builder
                        .current_block()
                        .expect("br_if requires an active block");

                    let fallthrough_block = builder.create_block();
                    // Note: In Molt IR, cond is 0 for false, !=0 for true.
                    // But brif takes a boolean condition (i32/i8 depending on type, Cranelift uses comparison result).
                    // We assume cond is already a boolean-like from cmp or we compare it to 0.
                    // Wait, `cond` from `vars` is I64 (NaN-boxed or raw int).
                    // We should check if it's truthy.
                    // But for now let's assume the frontend emits a boolean comparison result (0 or 1).
                    // Actually, let's play safe and check != 0.
                    let cond_bool = builder.ins().icmp_imm(IntCC::NotEqual, *cond, 0);

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
                        builder.ins().call(local_dec_ref, &[*val]);
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
                    let label_id = op.value.unwrap();
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
                    &entry_vars,
                    &last_use,
                    op_idx,
                );
                for val in cleanup {
                    builder.ins().call(local_dec_ref_obj, &[val]);
                }
                let cleanup =
                    drain_cleanup_entry_tracked(&mut tracked_vars, &entry_vars, &last_use, op_idx);
                for val in cleanup {
                    builder.ins().call(local_dec_ref, &[val]);
                }
            }

            if let Some(name) = out_name.as_ref()
                && name != "none"
                && let Some(block) = builder.current_block()
            {
                if block == entry_block && loop_depth == 0 {
                    if output_is_ptr {
                        tracked_vars.push(name.clone());
                    } else {
                        tracked_obj_vars.push(name.clone());
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
            for name in &tracked_vars {
                if let Some(val) = entry_vars.get(name) {
                    builder.ins().call(local_dec_ref, &[*val]);
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

        let id = self
            .module
            .declare_function(&func_ir.name, Linkage::Export, &self.ctx.func.signature)
            .unwrap();
        let define_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.module
                .define_function(id, &mut self.ctx)
                .map_err(Box::new)
        }));
        match define_result {
            Ok(Ok(())) => {}
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
                eprintln!("Backend panic while defining function {}", func_ir.name);
                if let Ok(filter) = std::env::var("MOLT_DUMP_CLIF")
                    && (filter == "1" || filter == func_ir.name || func_ir.name.contains(&filter))
                {
                    eprintln!("CLIF {}:\n{}", func_ir.name, self.ctx.func.display());
                }
                std::panic::resume_unwind(payload);
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
