use super::*;
use crate::repr::{ContainerKind, ContainerStorageKind, ScalarKind};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::runtime_import_abi::{MOLT_DEC_REF, MOLT_DEC_REF_OBJ, MOLT_INC_REF_OBJ};

// Per-op-family Cranelift codegen handlers lifted out of `compile_func_inner`
// (decomposition program M1). Scalar carrier/boxing helpers live in
// `scalar_carriers.rs` and stay visible only inside `function_compiler`, so the
// extracted `fc::*` families do not widen backend APIs.
mod scalar_carriers;
use scalar_carriers::*;
mod fc;
use fc::list_index_fast_path::{ListIndexFastPathState, loop_start_has_index_prelude};

mod cleanup_roots;
use cleanup_roots::*;
mod shared;
use shared::*;
mod block_control;
use block_control::*;
mod preanalysis;
use preanalysis::*;

#[cfg(feature = "native-backend")]
impl SimpleBackend {
    pub(crate) fn compile_func(
        &mut self,
        func_ir: FunctionIR,
        task_kinds: &BTreeMap<String, TrampolineKind>,
        task_closure_sizes: &BTreeMap<String, i64>,
        defined_functions: &BTreeSet<String>,
        module_known_functions: &BTreeSet<String>,
        closure_functions: &BTreeSet<String>,
        return_alias_summaries: &BTreeMap<String, crate::passes::ReturnAliasSummary>,
        emit_traces: bool,
        leaf_functions: &BTreeSet<String>,
        known_function_arities: &BTreeMap<String, usize>,
        function_has_ret: &BTreeMap<String, bool>,
    ) {
        let trace_compile = env_setting("MOLT_TRACE_COMPILE_FUNC")
            .as_deref()
            .map(parse_truthy_env)
            .unwrap_or(false);
        let compile_started = std::time::Instant::now();
        let trace_name = func_ir.name.clone();
        let trace_ops = func_ir.ops.len();
        let trace_params = func_ir.params.len();
        if trace_compile {
            eprintln!(
                "[molt-native-compile] start {} ops={} params={}",
                trace_name, trace_ops, trace_params
            );
            let _ = crate::debug_artifacts::append_debug_artifact(
                "native/compile_trace.txt",
                format!(
                    "start name={} ops={} params={}\n",
                    trace_name, trace_ops, trace_params
                ),
            );
        }
        if let Some(pattern) = env_setting("MOLT_DUMP_FINAL_FUNC_IR")
            && func_ir.name.contains(pattern.as_str())
        {
            let sanitized: String = func_ir
                .name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let mut dump = String::new();
            dump.push_str(&format!(
                "// final func: {} ({} ops)\n",
                func_ir.name,
                func_ir.ops.len()
            ));
            dump.push_str(&format!("// params: {:?}\n", func_ir.params));
            dump.push_str(&format!("// param_types: {:?}\n", func_ir.param_types));
            for (idx, op) in func_ir.ops.iter().enumerate() {
                dump.push_str(&format!(
                    "{:4}: kind={:30} out={:20} var={:20} args={:40} val={:?} sval={:?} fi={:?} ff={:?} stack={:?} task={:?} container={:?} type={:?} ic={:?}\n",
                    idx,
                    op.kind,
                    op.out.as_deref().unwrap_or(""),
                    op.var.as_deref().unwrap_or(""),
                    op.args.as_ref().map(|a| a.join(",")).unwrap_or_default(),
                    op.value,
                    op.s_value,
                    op.fast_int,
                    op.fast_float,
                    op.stack_eligible,
                    op.task_kind,
                    op.container_type,
                    op.type_hint,
                    op.ic_index,
                ));
            }
            let _ = crate::debug_artifacts::write_debug_artifact(
                format!("native/final_ir/{sanitized}.txt"),
                dump,
            );
        }
        self.compile_func_inner(
            func_ir,
            task_kinds,
            task_closure_sizes,
            defined_functions,
            module_known_functions,
            closure_functions,
            return_alias_summaries,
            emit_traces,
            leaf_functions,
            known_function_arities,
            function_has_ret,
        );
        if trace_compile {
            eprintln!(
                "[molt-native-compile] done {} after {:.2?}",
                trace_name,
                compile_started.elapsed()
            );
            let _ = crate::debug_artifacts::append_debug_artifact(
                "native/compile_trace.txt",
                format!(
                    "done name={} elapsed={:.2?}\n",
                    trace_name,
                    compile_started.elapsed()
                ),
            );
        }
    }

    /// Inner compilation for the current native backend path.
    pub(crate) fn compile_func_inner(
        &mut self,
        func_ir: FunctionIR,
        task_kinds: &BTreeMap<String, TrampolineKind>,
        task_closure_sizes: &BTreeMap<String, i64>,
        defined_functions: &BTreeSet<String>,
        module_known_functions: &BTreeSet<String>,
        closure_functions: &BTreeSet<String>,
        return_alias_summaries: &BTreeMap<String, crate::passes::ReturnAliasSummary>,
        emit_traces: bool,
        leaf_functions: &BTreeSet<String>,
        known_function_arities: &BTreeMap<String, usize>,
        function_has_ret: &BTreeMap<String, bool>,
    ) {
        {
            let ce_count = func_ir
                .ops
                .iter()
                .filter(|op| op.kind == "check_exception")
                .count();
            if std::env::var("MOLT_DEBUG_CHECK_EXC").is_ok()
                && (ce_count > 0
                    || func_ir.name.contains("molt_main")
                    || func_ir.name.contains("test_try"))
            {
                eprintln!(
                    "[COMPILE] func={} ops={} check_exception_count={}",
                    func_ir.name,
                    func_ir.ops.len(),
                    ce_count
                );
            }
        }
        let mut builder_ctx = FunctionBuilderContext::new();
        self.module.clear_context(&mut self.ctx);
        let representation_plan_storage = ScalarRepresentationPlan::for_function_ir(&func_ir);
        let representation_plan = &representation_plan_storage;
        let FunctionPreanalysis {
            has_ret,
            stateful,
            has_store,
            var_names,
            last_use,
            alias_roots,
            if_to_end_if,
            if_to_else,
            else_to_end_if,
            label_ids,
            state_label_ids: _state_label_ids,
            shared_resume_label_ids,
            state_ids: _state_ids,
            resume_states,
            function_exception_label_id,
            exception_label_ids,
            const_int_map: _const_int_map,
            loop_body_out_vars,
            loop_body_init_vars,
            has_arena_eligible,
            arena_eligible_outs: _arena_eligible_outs,
            scalar_slot_exclusion_unsafe,
            field_store_modes,
            drop_inserted,
        } = preanalyze_function_ir(&func_ir, return_alias_summaries, representation_plan);
        // RC drop-insertion substrate (design 20 §4.1, Phase 5): the SimpleIR-level
        // inc/dec coalescer (`rc_coalescing`) elides matched inc_ref/dec_ref PAIRS
        // it discovers in the op stream. For drop-inserted functions the TIR drop
        // pass is the sole RC authority and its `refcount_elim_post` step already
        // performed the sound (balance-preserving) elision at the TIR level; the
        // ad-hoc SimpleIR coalescer operates on the SAME `dec_ref`/`inc_ref` ops
        // and would wrongly null out a TIR-inserted loop-carried `DecRef(old)` it
        // mis-pairs with the slot-store transport's inc — re-opening the O(n)
        // accumulator leak. Retire it (empty skip sets) for those functions so the
        // TIR drops lower verbatim; the legacy native-RC functions keep it.
        let (rc_skip_inc, mut rc_skip_dec) = if drop_inserted {
            (HashSet::new(), HashSet::new())
        } else {
            crate::passes::compute_rc_coalesce_skips(&func_ir.ops, &last_use)
        };
        let rc_authority = NativeRcAuthority::from_drop_inserted(drop_inserted);
        let returns_value = has_ret || stateful;

        if returns_value {
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
            let var_type = if representation_plan.is_float_unboxed(name) {
                types::F64
            } else {
                types::I64
            };
            let var = builder.declare_var(var_type);
            vars.insert(name.clone(), var);
        }
        let mut first_defined_at: BTreeMap<String, usize> = BTreeMap::new();
        for name in func_ir.params.iter().filter(|name| name.as_str() != "none") {
            first_defined_at.entry(name.clone()).or_insert(0);
        }
        for (idx, op) in func_ir.ops.iter().enumerate() {
            if let Some(out) = op.out.as_ref()
                && out != "none"
            {
                first_defined_at.entry(out.clone()).or_insert(idx);
            }
            if matches!(op.kind.as_str(), "store_var" | "delete_var")
                && let Some(name) = op.var.as_ref().or(op.out.as_ref())
                && name != "none"
            {
                first_defined_at.entry(name.clone()).or_insert(idx);
            }
        }
        let trace_ops = should_trace_ops(&func_ir.name);
        let trace_stride = trace_ops.as_ref().map(|cfg| cfg.stride);
        let debug_loop_cfg = std::env::var("MOLT_DEBUG_LOOP_CFG")
            .ok()
            .filter(|raw| raw == "1" || func_ir.name.contains(raw));
        let debug_block_origins = std::env::var("MOLT_DEBUG_BLOCK_ORIGINS")
            .ok()
            .filter(|raw| raw == "1" || raw.as_str() == func_ir.name || func_ir.name.contains(raw));
        let debug_seal = std::env::var("MOLT_DEBUG_SEAL").as_deref() == Ok(func_ir.name.as_str());
        let maybe_debug_seal = |tag: &str, op_idx: usize, block: Block| {
            if debug_seal {
                let line = format!(
                    "SEAL_TRACE func={} tag={} op={} block={:?}\n",
                    func_ir.name, tag, op_idx, block
                );
                eprint!("{line}");
                if let Ok(path) = std::env::var("MOLT_DEBUG_SEAL_FILE")
                    && let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                {
                    let _ = std::io::Write::write_all(&mut file, line.as_bytes());
                }
            }
        };
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
        let mut tracked_vars_set: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut tracked_obj_vars_set: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut entry_vars: BTreeMap<String, Value> = BTreeMap::new();
        let mut label_blocks = BTreeMap::new();
        let mut resume_blocks = BTreeMap::new();
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
        // Global dedup set: tracks which variable names have already been
        // dec_ref'd by any cleanup site. Prevents double-free when tracked
        // values are cloned to multiple blocks by if/check_exception/br_if.
        let mut already_decrefed: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();

        // Phase 1d: int shadow plumbing eliminated. The main Cranelift
        // Variable IS the raw i64 carrier for representation_plan members.
        // Cranelift's FunctionBuilder inserts phi nodes automatically at
        // block boundaries when a Variable has multiple defs, so the legacy
        // two-tier shadow plumbing is redundant. Reading via
        // `int_raw_value(builder, vars, representation_plan, name)` returns the
        // raw i64 directly when name is a static member.

        // Phase 1d: representation_plan (declared above ~line 2665 via the
        // operand-recursive fixpoint) is the immutable source of truth for
        // "vars[name] holds raw i64". Float primary lowering follows the
        // same static-set rule for the F64-primary subset.
        // `representation_plan` is the immutable source of truth for F64-primary Variables.
        // Non-primary float values are boxed immediately in their main I64 Variable.
        let mut list_index_fast_paths = ListIndexFastPathState::default();
        let scalar_fast_paths_enabled = !is_cold_module_chunk_function(&func_ir.name);
        let entry_block = builder.create_block();
        let master_return_block = builder.create_block();
        if returns_value {
            builder.append_block_param(master_return_block, types::I64);
        }
        let entry_param_values: Vec<Value> = param_types
            .iter()
            .map(|ty| builder.append_block_param(entry_block, *ty))
            .collect();

        reachable_blocks.insert(entry_block);
        switch_to_block_materialized(&mut builder, entry_block);

        for (i, val) in entry_param_values.iter().copied().enumerate() {
            let name = &func_ir.params[i];
            def_var_named(&mut builder, &vars, name, val);
        }

        // Pre-declare shadow Variables for store_var targets whose source
        // is known to be an integer.  Only these need shadow tracking across
        // loop back-edges.  Pre-declaring ALL store_var targets would give
        // non-integer variables (sets, lists, strings) a bogus shadow of 0,
        // causing arithmetic operators to take the fast-int path and produce
        // garbage (e.g., set subtraction returning an int).
        let int_store_target_names = if scalar_fast_paths_enabled {
            let int_store_targets = representation_plan.scalar_store_targets(ScalarKind::Int);
            if std::env::var("MOLT_DUMP_INT_STORE_TARGETS").as_deref() == Ok(func_ir.name.as_str())
            {
                eprintln!("INT_STORE_TARGETS {} {:?}", func_ir.name, int_store_targets);
            }
            int_store_targets
        } else {
            BTreeSet::new()
        };
        // Only explicit store-backed join carriers and exception-fragile names
        // use stack slots. Structured phi joins must stay on the SSA path.
        // Plan-proven raw-int join slots are excluded:
        // their unboxed i64 values are carried correctly via SSA phi, and stack
        // slot load/store + inc_ref/dec_ref is pure overhead for inline values.
        //
        // CONSERVATIVE: a scalar-like variable is only safe to exclude when it
        // does NOT escape the local scope.  If it is passed to function calls,
        // stored to heap, returned, or has explicit refcount ops, the slot
        // mechanism is needed for refcount correctness at phi-join boundaries.
        let mut slot_backed_join_names =
            collect_slot_backed_join_names(&func_ir.ops, &exception_label_ids, stateful);
        // In functions with exception handling or stateful resume points,
        // keep ALL store_var targets slot-backed to prevent regalloc2
        // block-parameter explosion. Scalar exclusion is only safe when
        // blocks are eagerly sealed (no exception labels and not stateful),
        // because eager sealing resolves phi nodes incrementally without
        // creating massive block parameter lists.
        if scalar_fast_paths_enabled && exception_label_ids.is_empty() && !stateful {
            slot_backed_join_names.retain(|name| {
                let is_scalar = representation_plan.name_is_numeric_scalar(name);
                let is_safe_to_exclude = is_scalar && !scalar_slot_exclusion_unsafe.contains(name);
                !is_safe_to_exclude
            });
        }
        let mut slot_backed_join_slots: BTreeMap<String, cranelift_codegen::ir::StackSlot> =
            BTreeMap::new();
        if !slot_backed_join_names.is_empty() {
            for name in slot_backed_join_names.iter() {
                let slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let zero = builder.ins().iconst(types::I64, 0);
                builder.ins().stack_store(zero, slot, 0);
                slot_backed_join_slots.insert(name.clone(), slot);
            }
        }
        // Raw-backed join slots: int-primary names whose slot carries RAW i64
        // and bool-primary names whose slot carries RAW 0/1 (no NaN box, no
        // refcounting — a raw scalar is never a heap pointer).
        //
        // This is the single-carrier-convention completion of the primary
        // contracts: the name-keyed chain admits FULL-RANGE i64 carriers (the
        // overflow_peel accumulator cycle is the motivating case), so a
        // slot-backed transport that NaN-boxes on store and TRUSTED-unboxes
        // (`(v<<17)>>17`) on load would truncate any value past the 47-bit
        // inline window — the silent-integer-miscompile class. Carrying the
        // slot raw removes the hazard AND deletes the per-iteration
        // box/unbox/inc_ref/dec_ref churn every counted loop in an
        // exception-observing function previously paid. The bool lane (the
        // peel's loop-carried overflow flag) gets the same treatment so the
        // break-condition chain stays call-free.
        let raw_backed_slot_names: BTreeSet<String> = if scalar_fast_paths_enabled {
            slot_backed_join_slots
                .keys()
                .filter(|name| {
                    representation_plan.is_raw_int_carrier_name(name.as_str())
                        || representation_plan.is_bool_unboxed(name.as_str())
                })
                .cloned()
                .collect()
        } else {
            BTreeSet::new()
        };

        let _local_dec_ref = import_runtime_func_ref(
            &mut self.module,
            &mut self.import_ids,
            &mut builder,
            &mut import_refs,
            MOLT_DEC_REF,
        );
        let local_dec_ref_obj = import_runtime_func_ref(
            &mut self.module,
            &mut self.import_ids,
            &mut builder,
            &mut import_refs,
            MOLT_DEC_REF_OBJ,
        );
        let local_inc_ref_obj = import_runtime_func_ref(
            &mut self.module,
            &mut self.import_ids,
            &mut builder,
            &mut import_refs,
            MOLT_INC_REF_OBJ,
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
        // per function and keep it in a dedicated stack slot. Using a
        // Cranelift Variable here let SSA repair synthesize zero-valued
        // placeholder predecessors in nested if/check_exception shapes,
        // which could drop the live flag pointer on one edge and corrupt
        // exception propagation. A stack slot keeps the invariant pointer
        // available across arbitrary CFG without introducing block params.
        let has_exc_handling = function_exception_label_id.is_some();
        static INLINE_EXC_DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let inline_exc_disabled = *INLINE_EXC_DISABLED.get_or_init(|| {
            env_setting("MOLT_BACKEND_INLINE_EXC_DISABLED")
                .as_deref()
                .map(parse_truthy_env)
                .unwrap_or(false)
        });
        let exc_global_flag_ptr_fn = if has_exc_handling && !inline_exc_disabled {
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
        let exc_task_flag_ptr_fn = if has_exc_handling && !inline_exc_disabled {
            Some(import_func_ref(
                &mut self.module,
                &mut self.import_ids,
                &mut builder,
                &mut import_refs,
                "molt_task_exception_pending_flag_ptr",
                &[],
                &[types::I64],
            ))
        } else {
            None
        };
        let exc_flag_ptr_slot = if exc_global_flag_ptr_fn.is_some() {
            Some(builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                8,
                3,
            )))
        } else {
            None
        };
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

        let nbc = NanBoxConsts::new();

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
            if representation_plan.is_bool_unboxed(name) {
                let raw = vars.get(name).map(|&var| builder.use_var(var))?;
                return Some(crate::VarValue(box_raw_bool_value(builder, raw, &nbc)));
            }
            var_get_boxed_overflow_safe_base(
                module,
                import_ids,
                builder,
                import_refs,
                sealed_blocks,
                vars,
                name,
                representation_plan,
            )
        };

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
            let self_ptr = var_get_boxed_overflow_safe(
                &mut self.module,
                &mut self.import_ids,
                &mut builder,
                &mut import_refs,
                &mut sealed_blocks,
                &vars,
                "self",
                representation_plan,
            )
            .expect("Self not found");
            let self_bits = box_ptr_value(&mut builder, *self_ptr, &nbc);
            def_var_named(&mut builder, &vars, "self", self_bits);
        }

        let profile_enabled_val = local_profile_enabled.map(|local_profile_enabled| {
            let call = builder.ins().call(local_profile_enabled, &[]);
            builder.inst_results(call)[0]
        });

        // Fetch the exception flag pointer once in the entry block and keep
        // it in a stack slot so later check_exception sites can load it
        // without re-entering Cranelift SSA variable repair.
        if let (Some(slot), Some(global_fn_ref), Some(task_fn_ref)) = (
            exc_flag_ptr_slot,
            exc_global_flag_ptr_fn,
            exc_task_flag_ptr_fn,
        ) {
            let global_call = builder.ins().call(global_fn_ref, &[]);
            let global_ptr = builder.inst_results(global_call)[0];
            let task_call = builder.ins().call(task_fn_ref, &[]);
            let task_ptr = builder.inst_results(task_call)[0];
            let zero = builder.ins().iconst(types::I64, 0);
            let has_task_flag = builder.ins().icmp(IntCC::NotEqual, task_ptr, zero);
            let active_ptr = builder.ins().select(has_task_flag, task_ptr, global_ptr);
            builder.ins().stack_store(active_ptr, slot, 0);
        }

        // ── Entry-block variable initialization ──────────────────────────
        //
        // Cranelift requires every Variable to have a def_var that
        // dominates all uses.  The standard pattern is a blanket def_var
        // in the entry block.
        //
        // CRITICAL: box_none (0x7FFB — NaN-boxed None) as the entry-block
        // default corrupts Cranelift SSA phi resolution.  On the first
        // loop iteration, variables defined INSIDE the loop body resolve
        // through the dominator tree to the entry-block definition.  If
        // that definition is box_none, runtime functions receive None
        // instead of the intended value:
        //   • CONST 1 → None: eq(n, None) = False, break never fires
        //   • list_new → None: store_index(None, 0, v) = crash
        //   • const_str → None: module_get_attr(mod, None) = crash
        //
        // FIX: Variables defined inside or after the first loop get raw 0
        // (0x0000) as their entry-block default.  Raw 0 is:
        //   • Safe for dec_ref (non-pointer NaN tag → no-op)
        //   • Never mistaken for a valid Python object
        //   • Detectable as "uninitialized" by runtime checks
        //
        // Variables defined ONLY before any loop (or when no loops exist)
        // keep box_none because they are genuinely None-initialized
        // locals that exception handlers may read.
        // Detect whether the function contains any loop or back-edge.
        // After TIR optimization, structured loop markers (loop_start etc.)
        // are replaced with linearized label/jump/br_if ops.  A back-edge
        // exists when a jump or br_if targets a label defined earlier.
        let has_loop_or_backedge = {
            let mut defined_labels = std::collections::HashSet::new();
            let mut found = false;
            for op in &func_ir.ops {
                match op.kind.as_str() {
                    "loop_start" | "loop_index_start" | "for_iter_start" | "while_start"
                    | "async_for_start" => {
                        found = true;
                        break;
                    }
                    "label" | "state_label" => {
                        if let Some(id) = op.value {
                            defined_labels.insert(id);
                        }
                    }
                    "jump" | "br_if" | "loop_continue" => {
                        if let Some(id) = op.value
                            && defined_labels.contains(&id)
                        {
                            found = true;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            found
        };
        {
            // Functions with loops: use raw 0 for ALL non-param variables.
            // Functions without loops: use box_none for clean exception
            // handler semantics (undefined variables read as Python None).
            //
            // box_none (0x7FFB) is unsafe in loop-bearing functions because
            // Cranelift's SSA phi at loop headers picks the entry-block
            // definition as the reaching value on the first iteration.
            // Runtime functions then receive None instead of the intended
            // value (constants, heap pointers, comparison results).
            //
            // Raw 0 is safe: dec_ref no-ops, comparisons detect it as
            // non-equal to any valid object, is_truthy returns false.
            // In loop-bearing functions, pre-materialize constants in the
            // entry block so the entry-block def_var IS the correct value.
            // The phi at loop headers then picks the correct constant on
            // the first iteration instead of a bogus default.
            let const_int_defs: BTreeMap<String, i64> = if has_loop_or_backedge {
                fc::const_literals::collect_loop_entry_const_defs(&func_ir, representation_plan)
            } else {
                BTreeMap::new()
            };
            let none_val = builder.ins().iconst(types::I64, box_none());
            let float_zero = builder.ins().f64const(0.0);
            for (name, var) in &vars {
                if param_name_set.contains(name.as_str()) {
                    continue;
                }
                if representation_plan.is_float_unboxed(name) {
                    // Float-primary: Variable is F64, initialize with f64 zero.
                    builder.def_var(*var, float_zero);
                } else if representation_plan.is_raw_int_carrier_name(name) {
                    // Int-primary: the main Variable is raw i64, including
                    // entry pre-materialization for loop phis.
                    let raw = const_int_defs.get(name).copied().unwrap_or(0);
                    let val = builder.ins().iconst(types::I64, raw);
                    builder.def_var(*var, val);
                } else if let Some(&bits) = const_int_defs.get(name) {
                    // Pre-materialize constant in entry block so loop header
                    // phis pick up the correct value on the first iteration.
                    let val = builder.ins().iconst(types::I64, bits);
                    builder.def_var(*var, val);
                } else {
                    // Default to box_none (NaN-boxed Python None). This is
                    // safe for all runtime operations: is_truthy(None)=false,
                    // dec_ref(None)=no-op, type checks detect None correctly.
                    //
                    // NOTE: raw 0 is NOT safe here -- it's IEEE 754 float 0.0
                    // which breaks NaN-box type dispatch (to_i64 returns None,
                    // is_truthy returns false for wrong reasons, eq checks fail).
                    builder.def_var(*var, none_val);
                }
            }
        }

        // ── Heap-literal prologue hoisting ──────────────────────────────
        //
        // Hoist ALL immutable heap literals to the entry block. Each unique
        // string/bytes payload is allocated once and stored in a dedicated
        // stack slot. Subsequent const_str/const_bytes ops with the same
        // content load from the slot instead of re-allocating.
        //
        // This is the correct fix for loop-carried heap literals:
        // Cranelift SSA variables for heap constants can be corrupted to
        // None by loop-header phi merges (entry-block None init vs
        // back-edge value). Stack slots are immune to SSA phi because
        // they are physical memory, not SSA values. By allocating all
        // immutable heap literals before the entry block is sealed, their
        // object pointers are valid for the entire function lifetime.
        let literal_hoists = fc::const_literals::hoist_heap_literals(
            &func_ir,
            &mut self.module,
            &mut self.import_ids,
            &mut self.data_pool,
            &mut self.next_data_id,
            &mut builder,
            &vars,
            representation_plan,
        );

        // Traceback frame tracking is separate from full call tracing. The
        // frontend emits code-slot-backed trace_enter_slot/trace_exit markers
        // for every Python frame; native codegen lowers the enter marker at its
        // IR position so module code can initialize code slots first, then pops
        // exactly once in the unified return block.
        let has_frame_slot =
            emit_traces && func_ir.ops.iter().any(|op| op.kind == "trace_enter_slot");

        seal_block_once(&mut builder, &mut sealed_blocks, entry_block);
        sealed_blocks.insert(entry_block);

        // Keep textual control-flow labels and persisted resume states in
        // disjoint block maps. A numeric ready-continuation state may collide
        // with a regular label emitted later in the same function; only labels
        // that are themselves persisted as pending resume states share blocks.
        for label_id in label_ids {
            label_blocks
                .entry(label_id)
                .or_insert_with(|| builder.create_block());
        }
        for state_id in resume_states.iter().copied() {
            let block = if shared_resume_label_ids.contains(&state_id) {
                *label_blocks
                    .entry(state_id)
                    .or_insert_with(|| builder.create_block())
            } else {
                builder.create_block()
            };
            resume_blocks.insert(state_id, block);
        }
        let ops = &func_ir.ops;
        let mut label_join_slots: BTreeMap<i64, Vec<String>> = BTreeMap::new();
        let mut live_join_slots: BTreeSet<String> = BTreeSet::new();
        for op in ops {
            match op.kind.as_str() {
                "store_var" => {
                    if let Some(name) = op.var.as_ref()
                        && is_join_slot_name(name)
                    {
                        live_join_slots.insert(name.clone());
                    }
                }
                "load_var" => {
                    if let Some(name) = op.var.as_ref()
                        && is_join_slot_name(name)
                    {
                        live_join_slots.insert(name.clone());
                    }
                }
                "label" | "state_label" => {
                    if let Some(label_id) = op.value
                        && !live_join_slots.is_empty()
                    {
                        label_join_slots
                            .insert(label_id, live_join_slots.iter().cloned().collect());
                    }
                }
                _ => {}
            }
        }
        // 2. Implementation
        let mut skip_ops: BTreeSet<usize> = BTreeSet::new();
        let metadata_loop_ops = fc::loops::metadata_only_structured_loop_ops(ops);

        // -----------------------------------------------------------------
        // Scope arena lifecycle: MLKit/Cyclone region allocator integration.
        //
        // When escape analysis has marked any allocation in this function as
        // NoEscape (arena_eligible), emit a scope arena at function entry.
        // Arena-eligible allocs use molt_arena_alloc instead of molt_alloc,
        // and the arena is freed once at function exit instead of individual
        // per-object frees.
        // -----------------------------------------------------------------
        let scope_arena_ptr: Option<Value> = if has_arena_eligible {
            let arena_new = Self::import_func_id_split(
                &mut self.module,
                &mut self.import_ids,
                "molt_arena_new",
                &[],
                &[types::I64],
            );
            let local_arena_new = self.module.declare_func_in_func(arena_new, builder.func);
            let call = builder.ins().call(local_arena_new, &[]);
            Some(builder.inst_results(call)[0])
        } else {
            None
        };

        // Scalarized tuples: keep element SSA Values in a side table so
        // `len`/`index` can fold without touching the runtime. The tuple
        // object itself must still use the canonical runtime layout.
        let mut scalarized_tuples: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        if std::env::var("MOLT_DUMP_IR").as_deref() == Ok("ALL_OPS")
            && ops.iter().any(|o| o.kind == "not")
        {
            eprintln!("[FUNC] {} ({} ops)", func_ir.name, ops.len());
            for (i, op) in ops.iter().enumerate() {
                if op.kind.contains("not")
                    || op.kind.contains("bool")
                    || op.kind.contains("print")
                    || op.kind.contains("const")
                {
                    eprintln!(
                        "[OP] {}: kind={:20} out={:15?} args={:?} val={:?}",
                        i, op.kind, op.out, op.args, op.value
                    );
                }
            }
        }
        for op_idx in 0..ops.len() {
            if skip_ops.contains(&op_idx) || metadata_loop_ops.contains(&op_idx) {
                continue;
            }
            let op = ops[op_idx].clone();
            // Reconcile the logical block-filled flag with Cranelift's actual
            // block state before emitting any per-op instrumentation. Some
            // control-flow paths terminate the current block indirectly; if we
            // trust a stale `is_block_filled=false` here, the traceback
            // line/column update calls below can try to append instructions to
            // a filled block and panic.
            sync_block_filled(&builder, &mut is_block_filled);
            // Update frame stack column offsets for traceback carets when this
            // function has a code-slot-backed frame. Skip inside active loops;
            // line tracking follows the same hot-loop elision below.
            if has_frame_slot
                && !is_block_filled
                && loop_stack.is_empty()
                && let (Some(col_offset), Some(end_col_offset)) = (op.col_offset, op.end_col_offset)
            {
                let col_val = builder.ins().iconst(types::I64, col_offset);
                let end_col_val = builder.ins().iconst(types::I64, end_col_offset);
                let frame_line_col_fn = import_func_ref(
                    &mut self.module,
                    &mut self.import_ids,
                    &mut builder,
                    &mut import_refs,
                    "molt_frame_set_col",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                builder
                    .ins()
                    .call(frame_line_col_fn, &[col_val, end_col_val]);
            }
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
                // When is_block_filled is true, the current block has a terminator.
                // Instead of skipping ops (which leaves variables undefined and
                // breaks field access, exception stack, etc.), create a fresh
                // dead block so ops can execute harmlessly for SSA variable defs.
                // This replaces the whitelist approach that caused f.b = f.a bugs.
                if builder.current_block().is_none()
                    || block_has_terminator(&builder, builder.current_block().unwrap())
                {
                    let dead = builder.create_block();
                    switch_to_block_materialized(&mut builder, dead);
                    seal_block_once(&mut builder, &mut sealed_blocks, dead);
                }
                is_block_filled = false;
                // Fall through to the normal match — ops execute into the dead block
            }
            if !is_block_filled
                && let Some(stride) = trace_stride
                && op_idx % stride == 0
            {
                if std::env::var("MOLT_TRACE_OP_PROGRESS_STDERR").as_deref() == Ok("1") {
                    eprintln!(
                        "[molt-native-op] func={} op={} kind={} block={:?} filled={}",
                        func_ir.name,
                        op_idx,
                        op.kind,
                        builder.current_block(),
                        is_block_filled
                    );
                }
                if let (Some(name_var), Some(len_var), Some(trace_fn)) =
                    (trace_name_var, trace_len_var, trace_func)
                {
                    let name_bits = builder.use_var(name_var);
                    let len_bits = builder.use_var(len_var);
                    let idx_bits = builder.ins().iconst(types::I64, op_idx as i64);
                    builder
                        .ins()
                        .call(trace_fn, &[name_bits, len_bits, idx_bits]);
                }
            }
            // `store_var` defines the target slot just like `out`-producing ops
            // define their result name. Treat the destination variable as the
            // logical definition site so RC/liveness tracking preserves values
            // across structured joins emitted by the TIR roundtrip.
            let out_name = op.out.clone().or_else(|| {
                if matches!(op.kind.as_str(), "store_var" | "delete_var") {
                    op.var.clone()
                } else {
                    None
                }
            });
            let alias_src_name =
                preanalyze_alias_source(&ops[op_idx], return_alias_summaries).map(str::to_string);
            let mut output_is_ptr = false;

            let loop_reassign_old_val = fc::loops::capture_loop_reassign_old_value(
                &op,
                out_name.as_deref(),
                loop_depth,
                rc_authority,
                is_block_filled,
                &rc_skip_dec,
                &loop_body_out_vars,
                &vars,
                &mut builder,
            );

            // Single routing decision for this op, derived from each handler's
            // `HANDLED_KINDS` authority (see `fc::op_family`). The family arms
            // below guard on this instead of re-listing kinds, so the dispatch
            // can never drop a kind a handler owns — the 8b5773878 drift class is
            // unexpressible. `None` means an inline arm (below) or no native
            // codegen (handled by the loud catch-all).
            let op_family = fc::native_op_family(op.kind.as_str());
            match op.kind.as_str() {
                _ if op_family == Some(fc::NativeOpFamily::ConstLiterals) => {
                    let __flow = fc::const_literals::handle_const_literal_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut self.data_pool,
                        &mut self.next_data_id,
                        &mut builder,
                        &vars,
                        representation_plan,
                        &literal_hoists,
                        &mut rc_skip_dec,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // Arithmetic family (fc::arith), INCLUDING the 24 `vec_*`
                // reductions `handle_arith_op` delegates to `fc::vec_reductions`.
                // Both kind sets route here via `op_family`: the scalar authority
                // is `fc::arith::HANDLED_KINDS`, the reduction authority is
                // `fc::vec_reductions::HANDLED_KINDS`, and the dispatch table maps
                // both to `NativeOpFamily::Arith`. Dropping the dispatch's copy of
                // the `vec_*` list was the 8b5773878 regression (fixed 0323ad28c);
                // there is no longer a copy here to drop.
                _ if op_family == Some(fc::NativeOpFamily::Arith) => {
                    let __flow = fc::arith::handle_arith_op(
                        &op,
                        op_idx,
                        &func_ir.name,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &loop_stack,
                        scalar_fast_paths_enabled,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // Bitwise/shift binary operators are split from scalar add/sub/mul
                // so arithmetic lowering stays a set of smaller codegen units.
                _ if op_family == Some(fc::NativeOpFamily::BitwiseShift) => {
                    let __flow = fc::bitwise_shift::handle_bitwise_shift_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        scalar_fast_paths_enabled,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // Matrix operators are runtime-backed binary ops, not scalar
                // arithmetic; keep their routing authority out of fc::arith.
                _ if op_family == Some(fc::NativeOpFamily::MatrixOps) => {
                    let __flow = fc::matrix_ops::handle_matrix_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // Division/modulo/power/rounding arithmetic family extracted
                // from fc::arith so quotient/remainder codegen is its own
                // function-level rustc codegen unit and kind authority.
                _ if op_family == Some(fc::NativeOpFamily::ArithDivision) => {
                    let __flow = fc::arith_division::handle_arith_division_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        scalar_fast_paths_enabled,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // handle_sequence_op family - extracted to fc::sequence_ops (M1)
                _ if op_family == Some(fc::NativeOpFamily::Sequence) => {
                    let __flow = fc::sequence_ops::handle_sequence_op(
                        &op,
                        ops,
                        op_idx,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        &mut scalarized_tuples,
                        &mut skip_ops,
                        representation_plan,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // handle_generator_op family - extracted to fc::generators (M1)
                _ if op_family == Some(fc::NativeOpFamily::Generators) => {
                    fc::generators::handle_generator_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                _ if op_family == Some(fc::NativeOpFamily::ScalarBuiltins) => {
                    fc::scalar_builtins::handle_scalar_builtin(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_callargs_op family — extracted to fc::callargs (M1)
                _ if op_family == Some(fc::NativeOpFamily::Callargs) => {
                    let __flow = fc::callargs::handle_callargs_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // handle_list_op family — extracted to fc::list_ops (M1)
                _ if op_family == Some(fc::NativeOpFamily::ListOps) => {
                    let __flow = fc::list_ops::handle_list_op(
                        &op,
                        op_idx,
                        &func_ir.name,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                        local_inc_ref_obj,
                        &mut list_index_fast_paths,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // handle_dict_op family — extracted to fc::dict_ops (M1)
                _ if op_family == Some(fc::NativeOpFamily::DictOps) => {
                    let __flow = fc::dict_ops::handle_dict_op(
                        &op,
                        op_idx,
                        &func_ir.name,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        scalar_fast_paths_enabled,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // handle_set_op family — extracted to fc::set_ops (M1)
                _ if op_family == Some(fc::NativeOpFamily::SetOps) => {
                    let __flow = fc::set_ops::handle_set_op(
                        &op,
                        op_idx,
                        &func_ir.name,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // handle_indexing_op family - extracted to fc::indexing (M1)
                _ if op_family == Some(fc::NativeOpFamily::Indexing) => {
                    fc::indexing::handle_indexing_op(
                        &op,
                        op_idx,
                        &func_ir.name,
                        &func_ir.ops,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        &scalarized_tuples,
                        representation_plan,
                        &mut list_index_fast_paths,
                        scalar_fast_paths_enabled,
                        local_inc_ref_obj,
                        local_dec_ref_obj,
                        &nbc,
                    );
                }
                // handle_text_predicate family — extracted to fc::text_predicates (M1)
                _ if op_family == Some(fc::NativeOpFamily::TextPredicates) => {
                    fc::text_predicates::handle_text_predicate(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_text_transform family — extracted to fc::text_transform (M1)
                _ if op_family == Some(fc::NativeOpFamily::TextTransform) => {
                    fc::text_transform::handle_text_transform(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_runtime_op family - extracted to fc::runtime_ops (M1)
                _ if op_family == Some(fc::NativeOpFamily::RuntimeOps) => {
                    fc::runtime_ops::handle_runtime_op(
                        &op,
                        &func_ir.name,
                        is_block_filled,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        local_exc_pending_fast,
                        exc_flag_ptr_slot,
                        &nbc,
                    );
                }
                // handle_statistics_op family — extracted to fc::statistics (M1)
                _ if op_family == Some(fc::NativeOpFamily::Statistics) => {
                    fc::statistics::handle_statistics_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_type_conversion family — extracted to fc::type_conversions (M1)
                _ if op_family == Some(fc::NativeOpFamily::TypeConversions) => {
                    fc::type_conversions::handle_type_conversion(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_memoryview_buffer_op family — extracted to fc::memoryview_buffer (M1)
                _ if op_family == Some(fc::NativeOpFamily::MemoryviewBuffer) => {
                    fc::memoryview_buffer::handle_memoryview_buffer_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_dataclass_op family — extracted to fc::dataclass (M1)
                _ if op_family == Some(fc::NativeOpFamily::Dataclass) => {
                    fc::dataclass::handle_dataclass_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_compare_op family - extracted to fc::compare (M1)
                _ if op_family == Some(fc::NativeOpFamily::Compare) => {
                    fc::compare::handle_compare_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &loop_stack,
                        scalar_fast_paths_enabled,
                        &nbc,
                    );
                }
                // handle_unary_logic_op family - extracted to fc::unary_logic (M1)
                _ if op_family == Some(fc::NativeOpFamily::UnaryLogic) => {
                    let __flow = fc::unary_logic::handle_unary_logic_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        local_inc_ref_obj,
                        scalar_fast_paths_enabled,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // handle_parse_op family — extracted to fc::parse_ops (M1)
                _ if op_family == Some(fc::NativeOpFamily::ParseOps) => {
                    fc::parse_ops::handle_parse_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_coroutine_op family - extracted to fc::coroutine (M1)
                _ if op_family == Some(fc::NativeOpFamily::Coroutine) => {
                    let __flow = fc::coroutine::handle_coroutine_op(
                        &op,
                        ops,
                        op_idx,
                        entry_block,
                        master_return_block,
                        &resume_states,
                        &resume_blocks,
                        &label_blocks,
                        &mut reachable_blocks,
                        &mut is_block_filled,
                        rc_authority,
                        returns_value,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &mut block_tracked_obj,
                        &mut block_tracked_ptr,
                        &last_use,
                        &alias_roots,
                        &mut already_decrefed,
                        &entry_vars,
                        local_inc_ref_obj,
                        local_dec_ref_obj,
                        local_exc_pending_fast,
                        exc_flag_ptr_slot,
                        &maybe_debug_seal,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // handle_future_promise_op family — extracted to fc::future_promise (M1)
                _ if op_family == Some(fc::NativeOpFamily::FuturePromise) => {
                    fc::future_promise::handle_future_promise_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_funcobj_op family - extracted to fc::funcobj (M1)
                _ if op_family == Some(fc::NativeOpFamily::Funcobj) => {
                    let __flow = fc::funcobj::handle_funcobj_op(
                        &op,
                        op_idx,
                        emit_traces,
                        has_frame_slot,
                        is_block_filled,
                        rc_authority,
                        !loop_stack.is_empty(),
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        task_kinds,
                        task_closure_sizes,
                        defined_functions,
                        function_has_ret,
                        &mut self.trampoline_ids,
                        &mut self.declared_func_arities,
                        &mut local_closure_envs,
                        &mut block_tracked_obj,
                        &mut block_tracked_ptr,
                        &mut entry_vars,
                        &last_use,
                        &alias_roots,
                        &mut already_decrefed,
                        local_inc_ref_obj,
                        local_dec_ref_obj,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // handle_object_construct_op family — extracted to fc::object_construct (M1)
                _ if op_family == Some(fc::NativeOpFamily::ObjectConstruct) => {
                    fc::object_construct::handle_object_construct_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_gpu_intrinsic_op family - extracted to fc::funcobj (M1)
                _ if op_family == Some(fc::NativeOpFamily::GpuIntrinsic) => {
                    fc::funcobj::handle_gpu_intrinsic_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &vars,
                    );
                }
                // handle_call_op family - extracted to fc::calls (M1)
                _ if op_family == Some(fc::NativeOpFamily::Calls) => {
                    fc::calls::handle_call_op(
                        &op,
                        op_idx,
                        func_ir.name.as_str(),
                        emit_traces,
                        has_frame_slot,
                        returns_value,
                        rc_authority,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &param_name_set,
                        &first_defined_at,
                        &last_use,
                        &alias_roots,
                        module_known_functions,
                        closure_functions,
                        leaf_functions,
                        &local_closure_envs,
                        known_function_arities,
                        &self.declared_func_arities,
                        function_has_ret,
                        defined_functions,
                        return_alias_summaries,
                        &mut block_tracked_obj,
                        &mut block_tracked_ptr,
                        &mut tracked_obj_vars,
                        &mut tracked_vars,
                        &mut tracked_obj_vars_set,
                        &mut tracked_vars_set,
                        &mut entry_vars,
                        &mut already_decrefed,
                        local_inc_ref_obj,
                        local_dec_ref_obj,
                        &nbc,
                    );
                }
                // handle_value_transfer_op family - extracted to fc::value_transfer (M1)
                //
                // `copy` is grouped with the args-based alias ops here. The
                // frontend emits `{kind:"copy", args:[src], out:result}` (a
                // pure SSA value move), and `rewrite_copy_aliases`
                // (ir_rewrites.rs) collapses it to `nop` ONLY when neither
                // `out` nor `src` is a mutable-storage name — a `copy` whose
                // result/source is a reassigned local SURVIVES with kind
                // "copy" and reaches codegen. Omitting it routes the op to the
                // silent `_ => {}` arm below, which emits no codegen and leaves
                // the result SSA value undefined (resolving to the None
                // sentinel) — the same silent-miscompile class as the vec_*
                // dispatch drop fixed in 0323ad28c. `copy` shares the
                // args-based `identity_alias`/`binding_alias` lowering (result
                // = inc_ref'd alias of args[0]); the TIR ownership model
                // classifies all three as `CopyLowering::TransparentAlias`
                // (alias_analysis.rs), so the inc_ref + alias treatment is
                // RC-correct. WASM (wasm.rs) and Luau (luau.rs) group `copy`
                // with the alias ops the same way; native must not be the
                // asymmetric outlier. Keep in sync with the `copy` arm in
                // fc::value_transfer::handle_value_transfer_op.
                _ if op_family == Some(fc::NativeOpFamily::ValueTransfer) => {
                    fc::value_transfer::handle_value_transfer_op(
                        &op,
                        op_idx,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &mut block_tracked_obj,
                        &mut block_tracked_ptr,
                        &mut tracked_obj_vars,
                        &mut tracked_vars,
                        &mut tracked_obj_vars_set,
                        &mut tracked_vars_set,
                        &alias_roots,
                        &mut entry_vars,
                        &mut already_decrefed,
                        &rc_skip_inc,
                        local_inc_ref_obj,
                        local_dec_ref_obj,
                        &nbc,
                    );
                }
                // handle_module_op family — extracted to fc::modules (M1)
                _ if op_family == Some(fc::NativeOpFamily::Modules) => {
                    fc::modules::handle_module_op(
                        &op,
                        op_idx,
                        &func_ir.name,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                        local_inc_ref_obj,
                        literal_hoists.str_output_slots(),
                    );
                }
                // handle_class_op family — extracted to fc::class_ops (M1)
                _ if op_family == Some(fc::NativeOpFamily::ClassOps) => {
                    fc::class_ops::handle_class_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // Outlined class definition via molt_guarded_class_def
                // handle_type_check_op family — extracted to fc::type_checks (M1)
                _ if op_family == Some(fc::NativeOpFamily::TypeChecks) => {
                    fc::type_checks::handle_type_check_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_exception_op family — extracted to fc::exceptions (M1)
                _ if op_family == Some(fc::NativeOpFamily::Exceptions) => {
                    fc::exceptions::handle_exception_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_context_op family — extracted to fc::context_mgmt (M1)
                _ if op_family == Some(fc::NativeOpFamily::ContextMgmt) => {
                    fc::context_mgmt::handle_context_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_exception_stack_op family — extracted to fc::exception_stack (M1)
                _ if op_family == Some(fc::NativeOpFamily::ExceptionStack) => {
                    fc::exception_stack::handle_exception_stack_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_exception_control_op family - extracted to fc::exception_control (M1)
                _ if op_family == Some(fc::NativeOpFamily::ExceptionControl) => {
                    fc::exception_control::handle_exception_control_op(
                        &op,
                        op_idx,
                        &func_ir.name,
                        entry_block,
                        loop_depth,
                        &label_blocks,
                        &mut reachable_blocks,
                        &mut is_block_filled,
                        rc_authority,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &mut block_tracked_obj,
                        &mut block_tracked_ptr,
                        &mut tracked_obj_vars,
                        &mut tracked_vars,
                        &mut tracked_obj_vars_set,
                        &mut tracked_vars_set,
                        &last_use,
                        &alias_roots,
                        &mut already_decrefed,
                        &mut entry_vars,
                        local_dec_ref_obj,
                        local_exc_pending_fast,
                        exc_flag_ptr_slot,
                        &maybe_debug_seal,
                        &nbc,
                    );
                }
                // handle_file_io_op family — extracted to fc::file_io (M1)
                _ if op_family == Some(fc::NativeOpFamily::FileIo) => {
                    fc::file_io::handle_file_io_op(
                        &op,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                    );
                }
                // handle_control_flow_op family - extracted to fc::control_flow (M1)
                _ if op_family == Some(fc::NativeOpFamily::ControlFlow) => {
                    let __flow = fc::control_flow::handle_control_flow_op(
                        &op,
                        op_idx,
                        func_ir.name.as_str(),
                        &func_ir.ops,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &first_defined_at,
                        &last_use,
                        &alias_roots,
                        &if_to_else,
                        &if_to_end_if,
                        &else_to_end_if,
                        &int_store_target_names,
                        &exception_label_ids,
                        &list_index_fast_paths,
                        &mut block_tracked_obj,
                        &mut block_tracked_ptr,
                        &mut tracked_vars,
                        &mut tracked_obj_vars,
                        &mut tracked_vars_set,
                        &mut tracked_obj_vars_set,
                        &mut entry_vars,
                        &mut already_decrefed,
                        &mut reachable_blocks,
                        &mut if_stack,
                        &mut skip_ops,
                        &mut is_block_filled,
                        rc_authority,
                        scalar_fast_paths_enabled,
                        &maybe_debug_seal,
                        local_dec_ref_obj,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // handle_loop_op family - extracted to fc::loops (M1)
                _ if op_family == Some(fc::NativeOpFamily::Loops) => {
                    let __flow = fc::loops::handle_loop_op(
                        &op,
                        op_idx,
                        &func_ir,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &last_use,
                        &alias_roots,
                        &exception_label_ids,
                        &loop_body_init_vars,
                        &mut list_index_fast_paths,
                        &mut block_tracked_obj,
                        &mut block_tracked_ptr,
                        &entry_vars,
                        &mut already_decrefed,
                        &mut reachable_blocks,
                        &mut loop_stack,
                        &mut skip_ops,
                        &mut loop_depth,
                        &mut is_block_filled,
                        rc_authority,
                        scalar_fast_paths_enabled,
                        debug_loop_cfg.as_deref(),
                        debug_block_origins.as_deref(),
                        &maybe_debug_seal,
                        local_exc_pending_fast,
                        exc_flag_ptr_slot,
                        local_dec_ref_obj,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // handle_memory_op family - extracted to fc::memory (M1)
                _ if op_family == Some(fc::NativeOpFamily::Memory) => {
                    let __flow = fc::memory::handle_memory_op(
                        &op,
                        op_idx,
                        func_ir.name.as_str(),
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &param_name_set,
                        &last_use,
                        &alias_roots,
                        &field_store_modes,
                        &mut block_tracked_obj,
                        &mut block_tracked_ptr,
                        &mut entry_vars,
                        &mut already_decrefed,
                        defined_functions,
                        scope_arena_ptr,
                        &mut output_is_ptr,
                        stateful,
                        entry_block,
                        local_profile_struct,
                        profile_enabled_val,
                        local_inc_ref_obj,
                        local_dec_ref_obj,
                        rc_authority,
                        scalar_fast_paths_enabled,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // handle_attr_op family — extracted to fc::attrs (M1)
                _ if op_family == Some(fc::NativeOpFamily::Attrs) => {
                    let __flow = fc::attrs::handle_attr_op(
                        &op,
                        op_idx,
                        &func_ir.name,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &nbc,
                        local_inc_ref_obj,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // handle_ret_jump_op family - extracted to fc::ret_jump (M1)
                _ if op_family == Some(fc::NativeOpFamily::RetJump) => {
                    let __flow = fc::ret_jump::handle_ret_jump_op(
                        &op,
                        op_idx,
                        func_ir.name.as_str(),
                        &func_ir.ops,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        representation_plan,
                        &param_name_set,
                        &alias_roots,
                        &last_use,
                        &mut block_tracked_obj,
                        &mut block_tracked_ptr,
                        &mut tracked_vars,
                        &mut tracked_obj_vars,
                        &mut tracked_vars_set,
                        &mut tracked_obj_vars_set,
                        &mut entry_vars,
                        &mut already_decrefed,
                        &mut reachable_blocks,
                        &label_blocks,
                        &label_join_slots,
                        function_exception_label_id,
                        &slot_backed_join_slots,
                        &raw_backed_slot_names,
                        &list_index_fast_paths,
                        master_return_block,
                        &mut is_block_filled,
                        returns_value,
                        rc_authority,
                        scalar_fast_paths_enabled,
                        debug_block_origins.as_deref(),
                        &maybe_debug_seal,
                        local_inc_ref_obj,
                        local_dec_ref_obj,
                        &nbc,
                    );
                    match __flow {
                        fc::OpFlow::Continue => continue,
                        fc::OpFlow::Proceed => {}
                    }
                }
                // Loud single-source-of-truth backstop for the dispatch<->handler
                // mirror. Routing above is derived from each handler's
                // `HANDLED_KINDS` via `op_family`, so a handler's kind can never be
                // silently dropped from the dispatch (the 8b5773878 regression).
                // This arm catches the residual case: a result-producing kind that
                // NO inline arm and NO family claims. Leaving it unhandled would
                // leave its result SSA value undefined (resolving to the None
                // sentinel) -> the exact silent miscompile fixed in 0323ad28c. Fail
                // loud here, just as every fc::* handler's own `_ => unreachable!`.
                _ => {
                    if op.out.is_some()
                        && !fc::NATIVE_NO_CODEGEN_RESULT_KINDS.contains(&op.kind.as_str())
                    {
                        panic!(
                            "native backend: no codegen for result-producing op kind `{}` \
                             (out={:?}) in function `{}`. It is claimed by no inline dispatch \
                             arm and no fc::* family (HANDLED_KINDS) — the dispatch<->handler \
                             mirror drift class regressed by 8b5773878 / fixed 0323ad28c. Add \
                             the kind to the owning handler's HANDLED_KINDS, or to \
                             op_family::NATIVE_NO_CODEGEN_RESULT_KINDS if it legitimately needs \
                             no native codegen.",
                            op.kind, op.out, func_ir.name,
                        );
                    }
                }
            }

            fc::loops::emit_loop_reassign_old_drop(
                &mut builder,
                local_dec_ref_obj,
                loop_reassign_old_val,
                is_block_filled,
            );

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
            if std::env::var("MOLT_DEBUG_TRACKED_CLEANUP").as_deref() == Ok("1")
                && std::env::var("MOLT_DEBUG_FUNC_FILTER")
                    .ok()
                    .is_none_or(|f| func_ir.name.contains(&f))
            {
                let block = builder.current_block();
                let obj_tracked = block
                    .and_then(|b| block_tracked_obj.get(&b))
                    .cloned()
                    .unwrap_or_default();
                let ptr_tracked = block
                    .and_then(|b| block_tracked_ptr.get(&b))
                    .cloned()
                    .unwrap_or_default();
                let write_enabled = std::env::var("MOLT_DEBUG_OP_INDEX")
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
                    .is_none_or(|target| target == op_idx);
                if write_enabled {
                    let _ = crate::debug_artifacts::append_debug_artifact(
                        "native/tracked_cleanup_debug.txt",
                        format!(
                            "func={} op_idx={} kind={} block={:?} obj_tracked={:?} ptr_tracked={:?} entry_obj={:?} entry_ptr={:?}\n",
                            func_ir.name,
                            op_idx,
                            op.kind,
                            block,
                            obj_tracked,
                            ptr_tracked,
                            tracked_obj_vars,
                            tracked_vars,
                        ),
                    );
                }
            }
            if !is_block_filled && loop_depth == 0 && builder.current_block() == Some(entry_block) {
                let cleanup_skip = match op.kind.as_str() {
                    "call_func" | "call_bind" | "call_indirect" | "invoke_ffi" => op
                        .args
                        .as_ref()
                        .and_then(|args| args.first())
                        .map(String::as_str),
                    _ => None,
                };
                let cleanup = drain_cleanup_entry_tracked_with_authority(
                    rc_authority,
                    &mut tracked_obj_vars,
                    &mut entry_vars,
                    &last_use,
                    &alias_roots,
                    &mut already_decrefed,
                    op_idx,
                    cleanup_skip,
                );
                for val in cleanup {
                    builder.ins().call(local_dec_ref_obj, &[val]);
                }
                let cleanup = drain_cleanup_entry_tracked_with_authority(
                    rc_authority,
                    &mut tracked_vars,
                    &mut entry_vars,
                    &last_use,
                    &alias_roots,
                    &mut already_decrefed,
                    op_idx,
                    cleanup_skip,
                );
                for val in cleanup {
                    // Use dec_ref_obj (NaN-box aware) instead of dec_ref (raw ptr).
                    // entry_vars always stores NaN-boxed bits, not raw pointers,
                    // so we must use the variant that checks the tag before
                    // dereferencing.  Using raw dec_ref here would SIGSEGV for
                    // any non-pointer NaN-boxed value (floats, inline ints, etc.).
                    builder.ins().call(local_dec_ref_obj, &[val]);
                }
            }

            if !is_block_filled
                && let Some(dst_name) = out_name.as_ref()
                && dst_name != "none"
                && let Some(src_name) = alias_src_name.as_deref()
                && src_name != dst_name
            {
                let join_slot_transfer = op.kind == "store_var" && is_join_slot_name(dst_name);
                if join_slot_transfer {
                    let root = alias_roots
                        .get(src_name)
                        .map(String::as_str)
                        .unwrap_or(src_name);
                    if builder.current_block() == Some(entry_block) && loop_depth == 0 {
                        remove_tracked_alias_group(&mut tracked_vars, &alias_roots, root);
                        tracked_vars_set
                            .retain(|name| alias_roots.get(name).map(String::as_str) != Some(root));
                        remove_tracked_alias_group(&mut tracked_obj_vars, &alias_roots, root);
                        tracked_obj_vars_set
                            .retain(|name| alias_roots.get(name).map(String::as_str) != Some(root));
                        entry_vars.retain(|name, _| {
                            alias_roots.get(name).map(String::as_str) != Some(root)
                        });
                    } else if let Some(block) = builder.current_block() {
                        if let Some(tracked) = block_tracked_ptr.get_mut(&block) {
                            remove_tracked_alias_group(tracked, &alias_roots, root);
                        }
                        if let Some(tracked) = block_tracked_obj.get_mut(&block) {
                            remove_tracked_alias_group(tracked, &alias_roots, root);
                        }
                    }
                } else if last_use.get(src_name).copied() == Some(op_idx) {
                    if builder.current_block() == Some(entry_block) && loop_depth == 0 {
                        remove_tracked_name(&mut tracked_vars, src_name);
                        tracked_vars_set.remove(src_name);
                        remove_tracked_name(&mut tracked_obj_vars, src_name);
                        tracked_obj_vars_set.remove(src_name);
                        entry_vars.remove(src_name);
                    } else if let Some(block) = builder.current_block() {
                        if let Some(tracked) = block_tracked_ptr.get_mut(&block) {
                            remove_tracked_name(tracked, src_name);
                        }
                        if let Some(tracked) = block_tracked_obj.get_mut(&block) {
                            remove_tracked_name(tracked, src_name);
                        }
                    }
                }
            }

            if let Some(name) = out_name.as_ref()
                && name != "none"
                // RC drop-insertion substrate (design 20 §4.1, Phase 5): when the
                // TIR drop pass owns this function's RC, suppress heap-result
                // registration into the native value-tracking system entirely.
                // Registration is the SINGLE source that feeds every drain site
                // (`tracked_*`/`block_tracked_*`/`entry_vars` are populated nowhere
                // else), so skipping it here makes every `drain_cleanup_tracked_*`
                // call and the final-return cleanup loops no-ops — the TIR
                // `DecRef`/`IncRef` ops become the SOLE RC authority. Without this
                // the tracking holds a second reference on loop-carried
                // accumulators and the TIR `DecRef(old)` only takes rc 2→1, never
                // freeing it (the O(n) residual leak the activation must close).
                && rc_authority.native_value_tracking_enabled()
                && op.kind != "delete_var"
                && !slot_backed_join_slots.contains_key(name.as_str())
                && let Some(block) = builder.current_block()
                // RC coalescing: skip tracking for variables whose dec_ref
                // was elided because the matching inc_ref was also elided.
                && !rc_skip_dec.contains(name.as_str())
                // Parameters are borrowed from the caller — never track them
                // for cleanup dec_ref. The caller owns the reference.
                && !param_name_set.contains(name.as_str())
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
                    if let Some(val) = var_get_boxed_overflow_safe(
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        name,
                        representation_plan,
                    ) {
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
            if !rc_authority.native_value_tracking_enabled() {
                block_tracked_obj.clear();
                block_tracked_ptr.clear();
                tracked_vars.clear();
                tracked_obj_vars.clear();
                tracked_vars_set.clear();
                tracked_obj_vars_set.clear();
                entry_vars.clear();
            }
            // Both tracked_vars and tracked_obj_vars store NaN-boxed bits in
            // entry_vars, so always use dec_ref_obj (NaN-box aware) for cleanup.
            // Using raw dec_ref on NaN-boxed bits causes SIGSEGV for non-pointer
            // values (floats from abs/round, inline ints, etc.).
            for name in &tracked_vars {
                if cleanup_name_excluded(name, None, &param_name_set, representation_plan) {
                    continue;
                }
                if let Some(val) = entry_vars.get(name)
                    && mark_cleanup_root_once(&alias_roots, &mut already_decrefed, name)
                {
                    builder.ins().call(local_dec_ref_obj, &[*val]);
                }
            }
            for name in &tracked_obj_vars {
                if cleanup_name_excluded(name, None, &param_name_set, representation_plan) {
                    continue;
                }
                if let Some(val) = entry_vars.get(name)
                    && mark_cleanup_root_once(&alias_roots, &mut already_decrefed, name)
                {
                    builder.ins().call(local_dec_ref_obj, &[*val]);
                }
            }
            if returns_value {
                let none_bits = builder.ins().iconst(types::I64, box_none());
                jump_block(&mut builder, master_return_block, &[none_bits]);
            } else {
                jump_block(&mut builder, master_return_block, &[]);
            }
        }

        switch_to_block_materialized(&mut builder, master_return_block);
        seal_block_once(&mut builder, &mut sealed_blocks, master_return_block);

        if has_frame_slot {
            let trace_exit_fn = import_func_ref(
                &mut self.module,
                &mut self.import_ids,
                &mut builder,
                &mut import_refs,
                "molt_trace_exit",
                &[],
                &[types::I64],
            );
            builder.ins().call(trace_exit_fn, &[]);
        }

        // RC drop-insertion substrate (design 20 §4.1, Phase 5): the join-slot
        // exit-teardown is the memory-phi arm of the native value-tracking RC — it
        // releases each non-raw loop-carried slot's FINAL value at function exit
        // (Swift-ARC release-at-scope-exit). For drop-inserted functions the TIR
        // drops own this: the back-edge `DecRef(old)` releases each prior
        // iteration's value, and the loop-exit value is either dropped by the TIR
        // pass (dead on exit) or transferred to the caller by the return ABI (a
        // returned accumulator — design §1.2: "consumed by the return ABI; caller
        // dec-refs"). Running this teardown too would double-free the returned
        // accumulator (a use-after-free in the caller) — so it is suppressed and
        // the TIR drops are the sole authority.
        if rc_authority.native_value_tracking_enabled() {
            for (name, slot) in slot_backed_join_slots.iter() {
                // Raw-backed slots hold raw i64 scalars — never heap pointers,
                // never refcounted, nothing to release.
                if raw_backed_slot_names.contains(name) {
                    continue;
                }
                let val = builder.ins().stack_load(types::I64, *slot, 0);
                builder.ins().call(local_dec_ref_obj, &[val]);
            }
        }

        // -----------------------------------------------------------------
        // Scope arena teardown: free the arena before returning.
        // All bump-allocated (NoEscape) values are released in O(1).
        // -----------------------------------------------------------------
        if let Some(arena_ptr) = scope_arena_ptr {
            let arena_free = Self::import_func_id_split(
                &mut self.module,
                &mut self.import_ids,
                "molt_arena_free",
                &[types::I64],
                &[],
            );
            let local_arena_free = self.module.declare_func_in_func(arena_free, builder.func);
            builder.ins().call(local_arena_free, &[arena_ptr]);
        }

        let final_res = if returns_value {
            let res = builder.block_params(master_return_block)[0];
            Some(res)
        } else {
            None
        };

        // For molt_main: route the SUCCESS path through the runtime's
        // executable finalizer. The finalizer runs Python-level process-exit
        // hooks and then hard-exits, avoiding allocator/TLS destructor races.
        // On the EXCEPTION path, return normally so the C stub can print the
        // traceback before invoking the same finalizer with a failure code.
        if func_ir.name == "molt_main" {
            let has_exc = emit_exception_pending_condition(
                &mut builder,
                local_exc_pending_fast,
                exc_flag_ptr_slot,
            );

            let exit_block = builder.create_block();
            let normal_ret_block = builder.create_block();
            builder
                .ins()
                .brif(has_exc, normal_ret_block, &[], exit_block, &[]);

            // Success path: Python-level exit finalization + _exit(0).
            switch_to_block_materialized(&mut builder, exit_block);
            seal_block_once(&mut builder, &mut sealed_blocks, exit_block);
            let runtime_exit = Self::import_func_id_split(
                &mut self.module,
                &mut self.import_ids,
                "molt_runtime_exit",
                &[types::I64],
                &[types::I64],
            );
            let local_runtime_exit = self.module.declare_func_in_func(runtime_exit, builder.func);
            let zero = builder.ins().iconst(types::I64, 0);
            builder.ins().call(local_runtime_exit, &[zero]);
            // Unreachable after molt_runtime_exit, but Cranelift needs a terminator.
            builder
                .ins()
                .trap(cranelift_codegen::ir::TrapCode::user(1).unwrap());

            // Exception path: return normally for traceback printing.
            switch_to_block_materialized(&mut builder, normal_ret_block);
            seal_block_once(&mut builder, &mut sealed_blocks, normal_ret_block);
        }
        if let Some(res) = final_res {
            builder.ins().return_(&[res]);
        } else {
            builder.ins().return_(&[]);
        }

        // Zero-predecessor blocks are harmless dead code that Cranelift
        // skips during compilation.  Only log them when debugging.
        if std::env::var_os("MOLT_DUMP_CLIF_ON_CFG_ERROR").is_some() {
            let zero_pred_blocks = find_zero_pred_blocks(builder.func);
            if !zero_pred_blocks.is_empty() {
                eprintln!(
                    "Backend CFG issue in {}: zero-predecessor blocks {:?}",
                    func_ir.name, zero_pred_blocks
                );
                eprintln!("CLIF {}:\n{}", func_ir.name, builder.func.display());
            }
        }
        if let Ok(filter) = std::env::var("MOLT_DUMP_CLIF_FUNC")
            && (func_ir.name == filter || func_ir.name.contains(&filter))
        {
            eprintln!("CLIF {}:\n{}", func_ir.name, builder.func.display());
        }
        if let Ok(path) = std::env::var("MOLT_DUMP_CLIF_FILE")
            && let Ok(clif_filter) = std::env::var("MOLT_DUMP_CLIF_FILE_FILTER")
            && func_ir.name.contains(&clif_filter)
        {
            let clif_text = format!("CLIF {}:\n{}", func_ir.name, builder.func.display());
            let _ = std::fs::write(&path, &clif_text);
        }

        // Eliminate unreachable blocks BEFORE sealing.  Cranelift's SSA
        // builder can create alias cycles (v1 -> v2 -> v1) when use_var is
        // called in blocks that form unreachable loops.  These cycles cause
        // remove_constant_phis to assert (mismatched formals/actuals) and
        // alias_analysis to crash on empty blocks.  DFS from the entry block
        // and remove any blocks not visited — the canonical fix endorsed by
        // Cranelift maintainers (bytecodealliance/wasmtime#5022).
        //
        {
            let entry = builder.func.layout.entry_block().unwrap();
            let mut visited = BTreeSet::new();
            let mut stack = vec![entry];
            while let Some(block) = stack.pop() {
                if !visited.insert(block) {
                    continue;
                }
                // Collect successors from the terminator instruction
                if let Some(last_inst) = builder.func.layout.last_inst(block) {
                    // Branch destinations
                    for dest in builder.func.dfg.insts[last_inst].branch_destination(
                        &builder.func.dfg.jump_tables,
                        &builder.func.dfg.exception_tables,
                    ) {
                        stack.push(dest.block(&builder.func.dfg.value_lists));
                    }
                }
            }
            // Remove blocks not reachable from entry
            let all_blocks: Vec<_> = builder.func.layout.blocks().collect();
            for block in &all_blocks {
                let block = *block;
                if !visited.contains(&block) {
                    // Only insert traps into truly empty orphaned blocks —
                    // blocks that have no instructions AND are not known
                    // reachable from codegen.  For exception-handling
                    // functions, the DFS may miss blocks whose terminators
                    // are not yet wired (deferred sealing).  The
                    // `reachable_blocks` set protects those blocks.
                    if builder.func.layout.block_insts(block).next().is_none()
                        && !reachable_blocks.contains(&block)
                    {
                        switch_to_block_materialized(&mut builder, block);
                        builder
                            .ins()
                            .trap(cranelift_codegen::ir::TrapCode::user(1).unwrap());
                    }
                }
            }
            // ── Block-finalization invariant (fail-loud) ───────────────────────
            // Every block reached by the entry DFS above MUST carry a terminator
            // before `seal_all_blocks`/`finalize`. A DFS-reachable block left
            // empty is a structured-codegen bug: a predecessor's terminator
            // branches INTO it (that is how the DFS reached it), but the block
            // itself was never filled. Cranelift's downstream `unreachable_code`
            // pass does `last_inst(block).unwrap()` for every domtree-reachable
            // block, so such a block produces an opaque `unreachable_code.rs`
            // `Option::unwrap() on None` panic deep inside the backend. Surface it
            // here as an actionable molt-level diagnostic at the single
            // block-finalization authority, naming the function and block, so any
            // future regression of this class (e.g. a structured loop's
            // `after_block` orphaned when its `loop_end` is never emitted —
            // round-10's `while True: …; if c: break`) fails loud at the right
            // layer instead of crashing inside Cranelift. This is a verification
            // guard, not a workaround: the orphan must be fixed in codegen/lowering
            // (terminate the block), never papered over by trapping a reachable
            // block — that would change program semantics. Scoped to `visited`
            // (entry-reachable) blocks: those are exactly the ones Cranelift's
            // domtree pass dereferences; the trap loop above already handled the
            // unreachable-orphan case.
            for block in &all_blocks {
                let block = *block;
                if !visited.contains(&block) {
                    continue;
                }
                if builder.func.layout.block_insts(block).next().is_none() {
                    panic!(
                        "native codegen left REACHABLE block {block:?} empty (no terminator) \
                         in '{}': a predecessor branches to it but it was never filled. \
                         This is a structured control-flow lowering/codegen bug (e.g. a loop \
                         after_block or a break-cleanup block left unterminated); fix the \
                         block's terminator emission, do not trap it.",
                        func_ir.name,
                    );
                }
            }
        }
        builder.seal_all_blocks();
        builder.finalize();

        if let Some(config) = should_dump_ir()
            && dump_ir_matches(&config, &func_ir.name)
        {
            dump_ir_ops(&func_ir, &config.mode);
        }

        if std::env::var("MOLT_DEBUG_COMPILED_FUNCS").as_deref() == Ok("1") {
            let _ = crate::debug_artifacts::append_debug_artifact(
                "native/compiled_funcs.txt",
                format!("compiled: {}\n", func_ir.name),
            );
        }
        if let Ok(filter) = std::env::var("MOLT_DUMP_CLIF")
            && (filter == "1" || filter == func_ir.name || func_ir.name.contains(&filter))
        {
            let clif = format!("{}", self.ctx.func.display());
            eprintln!("CLIF {}:\n{}", func_ir.name, clif);
            let sanitized: String = func_ir
                .name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let _ =
                crate::debug_artifacts::write_debug_artifact(format!("clif/{sanitized}.txt"), clif);
        }

        let id = match self.module.declare_function(
            &func_ir.name,
            Linkage::Export,
            &self.ctx.func.signature,
        ) {
            Ok(id) => id,
            Err(e) => {
                let err_str = format!("{e}");
                if err_str.contains("IncompatibleSignature")
                    || err_str.contains("incompatible with previous declaration")
                {
                    panic!(
                        "declare_function signature mismatch for `{}`: {e}",
                        func_ir.name
                    );
                }
                panic!("declare_function failed for {}: {}", func_ir.name, e);
            }
        };
        // ── Deferred compilation ──────────────────────────────
        // Instead of compiling each function immediately, extract the
        // finalized Cranelift IR and push it onto the deferred list.
        // All deferred functions are compiled in parallel later via
        // flush_deferred_defines().  This avoids the sequential
        // bottleneck of Cranelift's register allocator and optimizer.
        let built_func =
            std::mem::replace(&mut self.ctx.func, cranelift_codegen::ir::Function::new());
        self.deferred_defines.push(crate::DeferredDefine {
            func_id: id,
            func: built_func,
            name: func_ir.name.clone(),
        });
        self.defined_func_names.insert(func_ir.name.clone());
        self.module.clear_context(&mut self.ctx);
    }
}

#[cfg(all(test, feature = "native-backend"))]
mod tests;
