use super::context::CompileFuncContext;
use super::op_loop::{ControlKind, WasmFunctionEmitContext};
use super::*;

fn emit_seeded_runtime_const_op(
    this: &mut WasmBackend,
    func: &mut Function,
    op: &OpIR,
    locals: &BTreeMap<String, u32>,
    func_index: u32,
    reloc_enabled: bool,
    import_ids: &TrackedImportIds,
    const_str_scratch_segment: DataSegmentRef,
) {
    match op.kind.as_str() {
        "const_not_implemented" => {
            emit_call(func, reloc_enabled, import_ids["not_implemented"]);
            let local_idx = locals[op.out.as_ref().expect("const_not_implemented out")];
            func.instruction(&Instruction::LocalSet(local_idx));
        }
        "const_ellipsis" => {
            emit_call(func, reloc_enabled, import_ids["ellipsis"]);
            let local_idx = locals[op.out.as_ref().expect("const_ellipsis out")];
            func.instruction(&Instruction::LocalSet(local_idx));
        }
        "const_str" => {
            let out_name = op.out.as_ref().expect("const_str out");
            let bytes = op
                .bytes
                .as_deref()
                .unwrap_or_else(|| op.s_value.as_ref().expect("const_str bytes").as_bytes());
            let data = this.add_data_segment(reloc_enabled, bytes);
            let ptr_local = locals[&format!("{out_name}_ptr")];
            let len_local = locals[&format!("{out_name}_len")];
            this.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::LocalSet(ptr_local));
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            func.instruction(&Instruction::LocalSet(len_local));
            func.instruction(&Instruction::LocalGet(ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(len_local));
            this.emit_data_ptr_i32(reloc_enabled, func_index, func, const_str_scratch_segment);
            emit_call(func, reloc_enabled, import_ids["string_from_bytes"]);
            func.instruction(&Instruction::Drop);
            let out_local = locals[out_name];
            this.emit_data_ptr_i32(reloc_enabled, func_index, func, const_str_scratch_segment);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::LocalSet(out_local));
        }
        "const_bigint" => {
            let s = op.s_value.as_ref().expect("const_bigint string");
            let out_name = op.out.as_ref().expect("const_bigint out");
            let bytes = s.as_bytes();
            let data = this.add_data_segment(reloc_enabled, bytes);
            let ptr_local = locals[&format!("{out_name}_ptr")];
            let len_local = locals[&format!("{out_name}_len")];
            this.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::LocalSet(ptr_local));
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            func.instruction(&Instruction::LocalSet(len_local));
            func.instruction(&Instruction::LocalGet(ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(len_local));
            emit_call(func, reloc_enabled, import_ids["bigint_from_str"]);
            let out_local = locals[out_name];
            func.instruction(&Instruction::LocalSet(out_local));
        }
        "const_bytes" => {
            let bytes = op.bytes.as_ref().expect("const_bytes bytes");
            let out_name = op.out.as_ref().expect("const_bytes out");
            let data = this.add_data_segment(reloc_enabled, bytes);
            let ptr_local = locals[&format!("{out_name}_ptr")];
            let len_local = locals[&format!("{out_name}_len")];
            this.emit_data_ptr(reloc_enabled, func_index, func, data);
            func.instruction(&Instruction::LocalSet(ptr_local));
            func.instruction(&Instruction::I64Const(bytes.len() as i64));
            func.instruction(&Instruction::LocalSet(len_local));
            func.instruction(&Instruction::LocalGet(ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(len_local));
            this.emit_data_ptr_i32(reloc_enabled, func_index, func, const_str_scratch_segment);
            emit_call(func, reloc_enabled, import_ids["bytes_from_bytes"]);
            func.instruction(&Instruction::Drop);
            let out_local = locals[out_name];
            this.emit_data_ptr_i32(reloc_enabled, func_index, func, const_str_scratch_segment);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::LocalSet(out_local));
        }
        _ => panic!("unsupported seeded runtime const op {}", op.kind),
    }
}

impl WasmBackend {
    pub(super) fn compile_func(
        &mut self,
        func_ir: &FunctionIR,
        type_idx: u32,
        ctx: &CompileFuncContext<'_>,
    ) {
        let func_index = self.func_count;
        let reloc_enabled = ctx.reloc_enabled;
        if std::env::var("MOLT_DEBUG_WASM_SIG_FUNC").ok().as_deref() == Some(func_ir.name.as_str())
        {
            eprintln!(
                "WASM_SIG_FUNC name={} type_idx={} params={:?} param_types={:?}",
                func_ir.name, type_idx, func_ir.params, func_ir.param_types
            );
        }
        self.funcs.function(type_idx);
        if reloc_enabled && func_ir.name == "molt_main" {
            self.molt_main_index = Some(func_index);
        } else {
            self.exports
                .export(&func_ir.name, ExportKind::Func, self.func_count);
        }
        self.func_count += 1;
        if is_production_lir_wasm_fast_path_name(&func_ir.name)
            && !ctx.escaped_callable_targets.contains(&func_ir.name)
            && let Some(lir_output) = ctx.lir_fast_outputs.get(&func_ir.name)
        {
            if std::env::var("MOLT_DEBUG_WASM_SIG_FUNC").ok().as_deref()
                == Some(func_ir.name.as_str())
            {
                eprintln!(
                    "WASM_SIG_FUNC fast_path name={} lir_param_types={:?} lir_result_types={:?}",
                    func_ir.name, lir_output.param_types, lir_output.result_types
                );
            }
            let mut func = Function::new_with_locals_types(lir_output.locals.clone());
            // Resolve NAMED runtime calls: the k-th placeholder pairs with
            // runtime_calls[k] (positional — instruction indexes shift under
            // the LIR peephole pass, so the pairing is by order, not index).
            let mut named_calls = lir_output.runtime_calls.iter();
            for instruction in &lir_output.instructions {
                if matches!(
                    instruction,
                    Instruction::Call(crate::tir::lower_to_wasm::NAMED_RUNTIME_CALL_PLACEHOLDER)
                ) {
                    let name = named_calls.next().unwrap_or_else(|| {
                        panic!(
                            "LIR fast output for '{}' has more named-call placeholders than runtime_calls entries",
                            func_ir.name
                        )
                    });
                    let import_index = ctx.import_ids[name];
                    assert!(
                        import_index != u32::MAX,
                        "LIR fast output for '{}' calls runtime import '{name}' which was skipped/pruned from the import set",
                        func_ir.name
                    );
                    func.instruction(&Instruction::Call(import_index));
                    continue;
                }
                func.instruction(instruction);
            }
            assert!(
                named_calls.next().is_none(),
                "LIR fast output for '{}' has unconsumed runtime_calls entries",
                func_ir.name
            );
            self.codes.function(&func);
            return;
        }
        let func_map = ctx.func_map;
        let func_indices = ctx.func_indices;
        let trampoline_map = ctx.trampoline_map;
        let table_base = ctx.table_base;
        let import_ids = ctx.import_ids;
        let closure_functions = ctx.closure_functions;
        let mut locals = BTreeMap::new();
        let mut local_count = 0;
        let mut local_types = Vec::new();

        for (idx, name) in func_ir.params.iter().enumerate() {
            locals.insert(name.clone(), idx as u32);
            local_count += 1;
        }

        if func_ir.name.ends_with("_poll") {
            let self_param_idx = locals.get("self").copied().unwrap_or(0);
            locals.insert("self_param".to_string(), self_param_idx);
            let self_idx = locals.get("self").copied();
            if self_idx.is_none() || self_idx == Some(self_param_idx) {
                locals.insert("self".to_string(), local_count);
                local_types.push(ValType::I64);
                local_count += 1;
            }
            if local_count == 0 {
                local_count = 1;
            }
        }

        // --- Dead local elimination: pre-scan to find which IR variables are
        // ever *read* (appear in op.args or op.var).  Output-only variables
        // that are never read can share a single WASM local ("dead sink"),
        // reducing the total local count and binary size.
        let read_vars: BTreeSet<String> = {
            let mut s = BTreeSet::new();
            for op in &func_ir.ops {
                if let Some(args) = &op.args {
                    for arg in args {
                        s.insert(arg.clone());
                    }
                }
                if let Some(var) = &op.var {
                    s.insert(var.clone());
                }
            }
            s
        };
        // Also treat function parameters as always live.
        let param_set: BTreeSet<String> = func_ir.params.iter().cloned().collect();
        let mut runtime_lookup_vars: BTreeSet<String> = BTreeSet::new();
        for op in &func_ir.ops {
            if op.kind == "builtin_func"
                && op.s_value.as_deref() == Some("molt_require_intrinsic_runtime")
                && let Some(out) = op.out.as_ref()
            {
                runtime_lookup_vars.insert(out.clone());
            }
        }
        let mut runtime_lookup_only_vars = runtime_lookup_vars.clone();
        for op in &func_ir.ops {
            if let Some(var) = op.var.as_ref()
                && runtime_lookup_vars.contains(var)
            {
                runtime_lookup_only_vars.remove(var);
            }
            if let Some(args) = op.args.as_ref() {
                for (idx, arg) in args.iter().enumerate() {
                    if !runtime_lookup_vars.contains(arg) {
                        continue;
                    }
                    let ok = op.kind == "call_func" && idx == 0 && args.len() == 3;
                    if !ok {
                        runtime_lookup_only_vars.remove(arg);
                    }
                }
            }
        }

        // --- Local variable coalescing (liveness analysis) ---
        // Compute live ranges for each variable: first write -> last read.
        // Variables whose ranges don't overlap can share a WASM local,
        // reducing total local count and binary size.
        let coalesced_map: BTreeMap<String, String> = if has_non_linear_control_flow(&func_ir.ops) {
            BTreeMap::new()
        } else {
            let mut first_write: BTreeMap<String, usize> = BTreeMap::new();
            let mut last_read: BTreeMap<String, usize> = BTreeMap::new();

            for (op_idx, op) in func_ir.ops.iter().enumerate() {
                if let Some(ref out) = op.out {
                    first_write.entry(out.clone()).or_insert(op_idx);
                }
                if let Some(ref args) = op.args {
                    for arg in args {
                        last_read.insert(arg.clone(), op_idx);
                    }
                }
                if let Some(ref var) = op.var {
                    last_read.insert(var.clone(), op_idx);
                }
            }

            // Build live ranges for coalescable temporaries only.
            // Only coalesce variables starting with __tmp or __v to be conservative.
            // Skip: parameters, dead-sink candidates (never read), _ptr/_len derivatives.
            let is_coalescable = |name: &str| -> bool {
                (name.starts_with("__tmp") || name.starts_with("__v"))
                    && !param_set.contains(name)
                    && read_vars.contains(name)
                    && !name.ends_with("_ptr")
                    && !name.ends_with("_len")
            };

            let mut ranges: Vec<(usize, usize, String)> = Vec::new();
            for (name, start) in &first_write {
                if !is_coalescable(name) {
                    continue;
                }
                let end = last_read.get(name).copied().unwrap_or(*start);
                ranges.push((*start, end, name.clone()));
            }

            // Sort by start position for greedy linear scan.
            ranges.sort_by_key(|r| r.0);

            // Greedy allocation: assign each variable to the lowest-numbered
            // "slot" (represented by the first variable that occupied it)
            // whose previous occupant's range has ended.
            // slot_end[i] = the end position of the variable currently in slot i.
            // slot_repr[i] = the representative variable name for slot i.
            let mut slot_end: Vec<usize> = Vec::new();
            let mut slot_repr: Vec<String> = Vec::new();
            let mut map: BTreeMap<String, String> = BTreeMap::new();

            for (start, end, name) in &ranges {
                // Find the lowest slot whose range has ended (end < start).
                let mut assigned = false;
                for (i, se) in slot_end.iter_mut().enumerate() {
                    if *se < *start {
                        // Reuse this slot: map this variable to the slot's representative.
                        *se = *end;
                        map.insert(name.clone(), slot_repr[i].clone());
                        assigned = true;
                        break;
                    }
                }
                if !assigned {
                    // Need a new slot; this variable is its own representative.
                    slot_end.push(*end);
                    slot_repr.push(name.clone());
                    map.insert(name.clone(), name.clone());
                }
            }

            map
        };

        // Allocate a single shared dead-sink local for output-only variables.
        let dead_sink_idx = local_count;
        locals.insert("__dead_sink".to_string(), dead_sink_idx);
        local_types.push(ValType::I64);
        local_count += 1;

        // ensure_local with dead-local awareness and coalescing: output-only
        // variables (never read) are mapped to the shared dead_sink_idx
        // instead of getting their own WASM local slot.  Coalescable
        // temporaries with non-overlapping lifetimes share locals via
        // the coalesced_map.  The `as_dead_out` flag indicates the caller
        // is allocating an output variable that should be checked against
        // the read set.
        let mut ensure_local_inner = |name: &str, as_dead_out: bool| -> u32 {
            if let Some(&idx) = locals.get(name) {
                return idx;
            }
            // Dead local elimination: if this is an output variable that
            // is never read and not a function parameter, reuse the
            // shared dead sink local.
            if as_dead_out && !read_vars.contains(name) && !param_set.contains(name) {
                locals.insert(name.to_string(), dead_sink_idx);
                return dead_sink_idx;
            }
            // Local coalescing: if this variable maps to a representative
            // that already has a local, reuse that local index.
            if let Some(repr) = coalesced_map.get(name)
                && repr != name
                && let Some(&repr_idx) = locals.get(repr)
            {
                locals.insert(name.to_string(), repr_idx);
                return repr_idx;
            }
            let idx = local_count;
            locals.insert(name.to_string(), idx);
            local_types.push(ValType::I64);
            local_count += 1;
            idx
        };

        let mut needs_field_fast = false;
        let mut needs_alloc_resolve = false;
        // Scope arena eligibility: any op marked `arena_eligible` triggers
        // a per-function ScopeArena lifecycle (arena_new at entry,
        // arena_alloc_object at every eligible alloc site, arena_free before
        // every return). Mirrors the native backend integration.
        let has_arena_eligible = func_ir.ops.iter().any(|op| op.arena_eligible == Some(true));
        let scalar_plan = ScalarRepresentationPlan::for_function_ir(func_ir);
        let mut stateful = false;
        let mut saw_jump_or_label = false;
        let mut fast_int_count: usize = 0;
        let mut const_seed_seen: BTreeSet<String> = BTreeSet::new();
        let mut const_seed_locals_all: Vec<(u32, i64)> = Vec::new();
        let mut seeded_runtime_const_ops: Vec<(usize, OpIR)> = Vec::new();
        let mut defined_vars: BTreeSet<String> = BTreeSet::new();
        let mut used_vars: BTreeSet<String> = BTreeSet::new();
        for op in &func_ir.ops {
            if let Some(args) = &op.args {
                for arg in args {
                    if arg != "self" && arg != "none" && arg.starts_with('v') {
                        used_vars.insert(arg.clone());
                    }
                }
            }
            if let Some(out) = &op.out
                && out != "none"
            {
                defined_vars.insert(out.clone());
            }
        }
        for (op_idx, op) in func_ir.ops.iter().enumerate() {
            if wasm_scalar_integer_fast_path_for_op(&scalar_plan, op) {
                fast_int_count += 1;
            }
            if let Some(var) = &op.var {
                let var_is_dead_out = op.kind == "store_var";
                ensure_local_inner(var, var_is_dead_out);
            }
            if let Some(args) = &op.args {
                for arg in args {
                    ensure_local_inner(arg, false);
                }
            }
            if let Some(out) = &op.out {
                let out_local_idx = ensure_local_inner(out, true);
                let is_dead = out_local_idx == dead_sink_idx;
                if op.kind == "const_str" || op.kind == "const_bytes" || op.kind == "const_bigint" {
                    // _ptr and _len locals are used internally by the op
                    // emission so they always need real (non-sink) locals.
                    ensure_local_inner(&format!("{out}_ptr"), false);
                    ensure_local_inner(&format!("{out}_len"), false);
                }
                if !const_seed_seen.contains(out) {
                    let bits = match op.kind.as_str() {
                        "const" => op.value.map(box_int),
                        "const_bool" => op.value.map(box_bool),
                        "const_float" => op.f_value.map(box_float),
                        "const_none" => Some(box_none()),
                        _ => None,
                    };
                    if let Some(bits) = bits {
                        // Skip seeding dead locals -- the value is never
                        // observed so there is no point initializing it.
                        if !is_dead {
                            const_seed_seen.insert(out.clone());
                            const_seed_locals_all.push((out_local_idx, bits));
                        }
                    } else if matches!(
                        op.kind.as_str(),
                        "const_str"
                            | "const_bytes"
                            | "const_bigint"
                            | "const_not_implemented"
                            | "const_ellipsis"
                    ) && !is_dead
                    {
                        const_seed_seen.insert(out.clone());
                        seeded_runtime_const_ops.push((op_idx, op.clone()));
                    }
                }
            }
            match op.kind.as_str() {
                "store" | "store_init" | "load" | "guarded_load" | "guarded_field_get"
                | "guarded_field_set" | "guarded_field_init" => needs_field_fast = true,
                "state_switch" | "state_transition" | "state_yield" | "chan_send_yield"
                | "chan_recv_yield" => stateful = true,
                "jump" | "label" => saw_jump_or_label = true,
                "alloc_task" => {
                    let tk = op.task_kind.as_deref().unwrap_or("future");
                    let has_prefix = tk == "generator";
                    let has_args = op.args.as_ref().is_some_and(|a| !a.is_empty());
                    if has_prefix || has_args {
                        needs_alloc_resolve = true;
                    }
                }
                _ => {}
            }
        }

        // Safety: seed undefined variables (used but never defined) with
        // box_none().  This can happen when front-end IR omits a const_none
        // definition due to module-context differences (e.g. genexpr compiled
        // for import vs __main__).  Without this, the WASM local defaults to
        // 0 which is not a valid boxed value and causes runtime crashes.
        for undef in used_vars.difference(&defined_vars) {
            if let Some(&local_idx) = locals.get(undef.as_str())
                && local_idx != dead_sink_idx
                && !param_set.contains(undef.as_str())
                && !const_seed_seen.contains(undef)
            {
                const_seed_seen.insert(undef.clone());
                const_seed_locals_all.push((local_idx, box_none()));
            }
        }

        if needs_field_fast {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                locals.entry("__wasm_tmp0".to_string())
            {
                entry.insert(local_count);
                local_types.push(ValType::I32);
                local_count += 1;
            }
            if let std::collections::btree_map::Entry::Vacant(entry) =
                locals.entry("__wasm_tmp1".to_string())
            {
                entry.insert(local_count);
                local_types.push(ValType::I64);
                local_count += 1;
            }
        }

        if needs_alloc_resolve
            && let std::collections::btree_map::Entry::Vacant(entry) =
                locals.entry("__wasm_alloc_resolve".to_string())
        {
            entry.insert(local_count);
            local_types.push(ValType::I32);
            local_count += 1;
        }

        // Reserve a slot to hold the ScopeArena handle returned by
        // `molt_arena_new`. The slot is initialised at function entry and
        // consumed by every arena-eligible alloc + every return site.
        let arena_local: Option<u32> = if has_arena_eligible {
            let idx = local_count;
            locals.insert("__wasm_scope_arena".to_string(), idx);
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };

        for name in ["__molt_tmp0", "__molt_tmp1", "__molt_tmp2", "__molt_tmp3"] {
            if let std::collections::btree_map::Entry::Vacant(entry) =
                locals.entry(name.to_string())
            {
                entry.insert(local_count);
                local_types.push(ValType::I64);
                local_count += 1;
            }
        }

        // Constant materialization cache: when a function body has 3+ fast_int
        // ops, pre-allocate WASM locals for the constants that would otherwise
        // be emitted as i64.const immediates dozens of times (INT_SHIFT,
        // INT_MIN_INLINE, INT_MAX_INLINE).  Below the threshold the overhead
        // of initializing the locals exceeds the savings.
        let const_cache = if fast_int_count >= 3 {
            let shift_idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            let min_idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            let max_idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            ConstantCache {
                int_shift: Some(shift_idx),
                int_min: Some(min_idx),
                int_max: Some(max_idx),
                ..ConstantCache::default()
            }
        } else {
            ConstantCache::default()
        };

        // Extended constant cache: cache box_none(), QNAN_TAG_MASK_I64, and
        // QNAN_TAG_PTR_I64 into locals unconditionally — these large i64
        // constants (9-10 bytes each as immediates) appear dozens of times in
        // every function body.  Replacing with local.get (1-2 bytes) saves
        // 7-8 bytes per occurrence.
        let const_cache = {
            let mut cc = const_cache;
            let none_idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            let mask_idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            let ptr_idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            cc.none_bits = Some(none_idx);
            cc.qnan_tag_mask = Some(mask_idx);
            cc.qnan_tag_ptr = Some(ptr_idx);
            cc
        };

        let jumpful = !stateful && saw_jump_or_label;

        // --- Tail call optimization eligibility (WASM tail calls proposal §3.5) ---
        // A function is eligible for tail call optimization when it is
        // non-stateful (stateful dispatch emits ops one-at-a-time).
        // Exception handling is checked per-call-site via try_stack
        // instead of blanket-disabling the whole function.
        let tail_call_eligible = !stateful;

        if stateful && !locals.contains_key("self_param") {
            let self_param_idx = locals
                .get("self")
                .copied()
                .or_else(|| {
                    func_ir
                        .params
                        .first()
                        .and_then(|name| locals.get(name))
                        .copied()
                })
                .unwrap_or_else(|| {
                    panic!(
                        "stateful wasm function {} missing self parameter",
                        func_ir.name
                    )
                });
            locals.insert("self_param".to_string(), self_param_idx);
            locals.entry("self".to_string()).or_insert(self_param_idx);
        }
        let self_ptr_local = if stateful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let state_local = if stateful || jumpful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let block_map_base_local = if stateful || jumpful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let return_local = if stateful || jumpful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let state_remap_base_local = if stateful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let state_remap_value_local = if stateful {
            let idx = local_count;
            local_types.push(ValType::I64);
            local_count += 1;
            Some(idx)
        } else {
            None
        };
        let const_seed_locals = if stateful || jumpful {
            const_seed_locals_all
        } else {
            Vec::new()
        };
        let seeded_runtime_const_ops = if stateful || jumpful {
            seeded_runtime_const_ops
        } else {
            Vec::new()
        };
        let seeded_runtime_const_op_indices: BTreeSet<usize> = seeded_runtime_const_ops
            .iter()
            .map(|(idx, _)| *idx)
            .collect();
        if std::env::var("MOLT_DEBUG_WASM_SEEDS_FUNC").ok().as_deref()
            == Some(func_ir.name.as_str())
        {
            eprintln!(
                "WASM_SEEDS_FUNC name={} seeds={:?} runtime_const_ops={}",
                func_ir.name,
                const_seed_locals,
                seeded_runtime_const_ops.len()
            );
            for name in &func_ir.params {
                if let Some(idx) = locals.get(name) {
                    eprintln!("WASM_SEEDS_PARAM name={} slot={}", name, idx);
                }
            }
            let mut slot_to_names: BTreeMap<u32, Vec<String>> = BTreeMap::new();
            for (name, &idx) in &locals {
                slot_to_names.entry(idx).or_default().push(name.clone());
            }
            for (slot, _) in &const_seed_locals {
                if let Some(names) = slot_to_names.get(slot) {
                    eprintln!("WASM_SEEDS_SLOT slot={} names={:?}", slot, names);
                }
            }
        }

        // --- Multi-value return optimization locals (Section 3.1) ---
        let multi_return_candidates = ctx.multi_return_candidates;
        let is_multi_return_callee = multi_return_candidates.get(&func_ir.name).copied();

        let mut multi_ret_locals: Vec<u32> = Vec::new();
        let mut multi_ret_tuple_vars: BTreeSet<String> = BTreeSet::new();
        if let Some(ret_count) = is_multi_return_callee {
            for i in 0..ret_count {
                let name = format!("__multi_ret_{i}");
                if let std::collections::btree_map::Entry::Vacant(e) = locals.entry(name) {
                    e.insert(local_count);
                    local_types.push(ValType::I64);
                    multi_ret_locals.push(local_count);
                    local_count += 1;
                }
            }
            for op in &func_ir.ops {
                if op.kind == "tuple_new"
                    && let Some(args) = &op.args
                    && args.len() == ret_count
                    && let Some(out) = &op.out
                {
                    multi_ret_tuple_vars.insert(out.clone());
                }
            }
        }

        let mut multi_ret_call_locals: BTreeMap<(String, i64), u32> = BTreeMap::new();
        let mut multi_ret_call_vars: BTreeSet<String> = BTreeSet::new();
        for (op_idx, op) in func_ir.ops.iter().enumerate() {
            if op.kind != "call_internal" {
                continue;
            }
            let Some(callee) = op.s_value.as_ref() else {
                continue;
            };
            let Some(&ret_count) = multi_return_candidates.get(callee) else {
                continue;
            };
            let Some(result_var) = op.out.as_ref() else {
                continue;
            };
            let mut valid = true;
            for k in 0..ret_count {
                let j = op_idx + 1 + k;
                if j >= func_ir.ops.len() {
                    valid = false;
                    break;
                }
                let next_op = &func_ir.ops[j];
                if next_op.kind != "tuple_index" {
                    valid = false;
                    break;
                }
                let Some(args) = next_op.args.as_ref() else {
                    valid = false;
                    break;
                };
                if args.len() < 2 || args[0] != *result_var {
                    valid = false;
                    break;
                }
            }
            if !valid {
                continue;
            }
            multi_ret_call_vars.insert(result_var.clone());
            for k in 0..ret_count {
                let name = format!("__multi_call_{result_var}_{k}");
                if !locals.contains_key(&name) {
                    locals.insert(name.clone(), local_count);
                    local_types.push(ValType::I64);
                    local_count += 1;
                }
                multi_ret_call_locals.insert((result_var.clone(), k as i64), locals[&name]);
            }
        }

        let _ = local_count;
        let mut func = Function::new_with_locals_types(local_types);
        if std::env::var("MOLT_DEBUG_WASM_LOCALS_FUNC").ok().as_deref()
            == Some(func_ir.name.as_str())
        {
            eprintln!("WASM_DEBUG_FUNC {}", func_ir.name);
            for (idx, op) in func_ir.ops.iter().enumerate() {
                let mut mentioned: Vec<String> = Vec::new();
                if let Some(args) = &op.args {
                    mentioned.extend(args.iter().cloned());
                }
                if let Some(var) = &op.var {
                    mentioned.push(var.clone());
                }
                if let Some(out) = &op.out {
                    mentioned.push(out.clone());
                }
                mentioned.sort();
                mentioned.dedup();
                let mapped: Vec<String> = mentioned
                    .into_iter()
                    .filter_map(|name| locals.get(&name).map(|slot| format!("{name}->{slot}")))
                    .collect();
                eprintln!(
                    "WASM_DEBUG_OP {} kind={} var={:?} out={:?} args={:?} locals={:?}",
                    idx, op.kind, op.var, op.out, op.args, mapped
                );
            }
        }
        let mut control_stack: Vec<ControlKind> = Vec::new();
        let mut try_stack: Vec<usize> = Vec::new();
        let mut label_stack: Vec<i64> = Vec::new();
        let mut label_depths: BTreeMap<i64, usize> = BTreeMap::new();

        let dispatch_blocks = if stateful || jumpful {
            let (block_starts, block_for_op) = build_dispatch_blocks(&func_ir.ops);
            let block_map_bytes = build_dispatch_block_map(&block_for_op);
            let block_map_segment = self.add_data_segment(reloc_enabled, &block_map_bytes);
            Some((block_starts, block_map_segment))
        } else {
            None
        };
        let dispatch_control_maps = if stateful || jumpful {
            Some(build_dispatch_control_maps(
                &func_ir.ops,
                stateful,
                &func_ir.name,
            ))
        } else {
            None
        };
        let state_resume_maps = if stateful {
            let (state_map, const_ints) = build_state_resume_maps(&func_ir.ops);
            let state_remap_table = build_dense_state_remap_table(&state_map).map(|remap_bytes| {
                let remap_entries = (remap_bytes.len() / std::mem::size_of::<i64>()) as i64;
                let remap_segment = self.add_data_segment(reloc_enabled, &remap_bytes);
                (remap_entries, remap_segment)
            });
            Some((state_map, const_ints, state_remap_table))
        } else {
            None
        };
        if let Some((_, block_map_segment)) = dispatch_blocks.as_ref() {
            let block_map_base_local =
                block_map_base_local.expect("block map base local missing for dispatch");
            self.emit_data_ptr(reloc_enabled, func_index, &mut func, *block_map_segment);
            func.instruction(&Instruction::LocalSet(block_map_base_local));
        }
        if let Some((_, _, Some((_, remap_segment)))) = state_resume_maps.as_ref() {
            let remap_base_local =
                state_remap_base_local.expect("state remap base local missing for stateful wasm");
            self.emit_data_ptr(reloc_enabled, func_index, &mut func, *remap_segment);
            func.instruction(&Instruction::LocalSet(remap_base_local));
        }
        if stateful || jumpful {
            for (_, op) in &seeded_runtime_const_ops {
                emit_seeded_runtime_const_op(
                    self,
                    &mut func,
                    op,
                    &locals,
                    func_index,
                    reloc_enabled,
                    import_ids,
                    ctx.const_str_scratch_segment,
                );
            }
            // Seed dispatch locals from their first literal assignment so control-flow
            // edge threading cannot observe a raw wasm zero (0.0 bits) for an
            // otherwise integer/none local before its defining block executes.
            for (local_idx, bits) in const_seed_locals.iter().copied() {
                func.instruction(&Instruction::I64Const(bits));
                func.instruction(&Instruction::LocalSet(local_idx));
            }
        }

        // Initialize constant materialization cache (once per function entry).
        const_cache.emit_init(&mut func);

        // Scope arena setup: invoke `molt_arena_new` once at function entry
        // and stash the handle in the reserved local. Mirrors the native
        // backend's MLKit-style region lifecycle so NoEscape allocations
        // bypass the global allocator and the entire arena is freed in O(1)
        // before each return.
        if let Some(idx) = arena_local {
            emit_call(&mut func, reloc_enabled, import_ids["arena_new"]);
            func.instruction(&Instruction::LocalSet(idx));
        }

        // Capture native_eh_enabled before the closure to avoid borrowing self.
        // Native EH requires non-relocatable output (wasm-ld doesn't support EH relocations)
        let native_eh_enabled = self.options.native_eh_enabled && !self.options.reloc_enabled;

        // Tail call optimization counter (WASM tail calls proposal §3.5).
        // Uses Cell so the closure can mutate it while also being borrowed
        // by multiple call sites (stateful dispatch emits ops one-at-a-time).
        let tail_call_count: Cell<usize> = Cell::new(0);

        let exception_handler_region_indices: BTreeSet<usize> = {
            let mut label_to_op_index: BTreeMap<i64, usize> = BTreeMap::new();
            for (idx, op) in func_ir.ops.iter().enumerate() {
                if matches!(op.kind.as_str(), "label" | "state_label")
                    && let Some(label_id) = op.value
                {
                    label_to_op_index.insert(label_id, idx);
                }
            }

            let mut regions = BTreeSet::new();
            let handler_labels: Vec<i64> = func_ir
                .ops
                .iter()
                .filter_map(|op| (op.kind == "check_exception").then_some(op.value).flatten())
                .collect();

            for label in handler_labels {
                let Some(&start_idx) = label_to_op_index.get(&label) else {
                    continue;
                };
                let mut nested_pushes = 0usize;
                for handler_idx in start_idx..func_ir.ops.len() {
                    let handler_op = &func_ir.ops[handler_idx];
                    regions.insert(handler_idx);
                    match handler_op.kind.as_str() {
                        "exception_push" => nested_pushes += 1,
                        "exception_pop" => {
                            if nested_pushes == 0 {
                                break;
                            }
                            nested_pushes -= 1;
                        }
                        "ret" | "ret_void" => break,
                        _ => {}
                    }
                }
            }
            regions
        };

        let mut op_emitter = WasmFunctionEmitContext {
            backend: self,
            func_ir,
            ctx,
            func_map,
            func_indices,
            trampoline_map,
            table_base,
            import_ids,
            closure_functions,
            runtime_lookup_only_vars: &runtime_lookup_only_vars,
            seeded_runtime_const_op_indices: &seeded_runtime_const_op_indices,
            exception_handler_region_indices: &exception_handler_region_indices,
            locals: &locals,
            const_cache: &const_cache,
            scalar_plan: &scalar_plan,
            multi_return_candidates,
            is_multi_return_callee,
            multi_ret_locals: &multi_ret_locals,
            multi_ret_tuple_vars: &multi_ret_tuple_vars,
            multi_ret_call_locals: &multi_ret_call_locals,
            multi_ret_call_vars: &multi_ret_call_vars,
            func_index,
            reloc_enabled,
            native_eh_enabled,
            tail_call_eligible,
            arena_local,
            tail_call_count: &tail_call_count,
        };

        if stateful {
            let func = &mut func;
            let state_local = state_local.expect("state local missing for stateful wasm");
            let self_ptr_local = self_ptr_local.expect("self ptr local missing for stateful wasm");
            let self_param = *locals
                .get("self_param")
                .expect("self_param missing for stateful wasm");
            let self_local = *locals
                .get("self")
                .expect("self local missing for stateful wasm");
            let op_count = func_ir.ops.len();
            let (block_starts, _) = dispatch_blocks
                .as_ref()
                .expect("dispatch blocks missing for stateful wasm");
            let block_count = block_starts.len();
            let block_map_base_local =
                block_map_base_local.expect("block map base local missing for stateful wasm");
            let dispatch_control_maps = dispatch_control_maps
                .as_ref()
                .expect("dispatch control maps missing for stateful wasm");
            let label_to_index = &dispatch_control_maps.label_to_index;
            let else_for_if = &dispatch_control_maps.else_for_if;
            let end_for_if = &dispatch_control_maps.end_for_if;
            let end_for_else = &dispatch_control_maps.end_for_else;
            let loop_continue_target = &dispatch_control_maps.loop_continue_target;
            let loop_break_target = &dispatch_control_maps.loop_break_target;
            let exception_handler_region_indices: std::collections::BTreeSet<usize> = {
                let mut regions = std::collections::BTreeSet::new();
                let handler_labels: Vec<i64> = func_ir
                    .ops
                    .iter()
                    .filter_map(|op| (op.kind == "check_exception").then_some(op.value).flatten())
                    .collect();
                for label in handler_labels {
                    let Some(&start_idx) = label_to_index.get(&label) else {
                        continue;
                    };
                    let mut nested_pushes = 0usize;
                    for handler_idx in start_idx..op_count {
                        let handler_op = &func_ir.ops[handler_idx];
                        regions.insert(handler_idx);
                        match handler_op.kind.as_str() {
                            "exception_push" => nested_pushes += 1,
                            "exception_pop" => {
                                if nested_pushes == 0 {
                                    break;
                                }
                                nested_pushes -= 1;
                            }
                            "ret" | "ret_void" => break,
                            _ => {}
                        }
                    }
                }
                regions
            };
            let (state_map, const_ints, state_remap_table) = state_resume_maps
                .as_ref()
                .expect("state resume maps missing for stateful wasm");
            let state_remap_table_entries = state_remap_table.as_ref().map(|(entries, _)| *entries);
            let sparse_state_remap_entries = state_remap_table_entries
                .is_none()
                .then(|| build_sparse_state_remap_entries(state_map));

            func.instruction(&Instruction::LocalGet(self_param));
            func.instruction(&Instruction::LocalSet(self_ptr_local));

            func.instruction(&Instruction::LocalGet(self_param));
            func.instruction(&Instruction::I64Const(POINTER_MASK as i64));
            func.instruction(&Instruction::I64And);
            const_cache.emit_qnan_tag_ptr(func);
            func.instruction(&Instruction::I64Or);
            func.instruction(&Instruction::LocalSet(self_local));

            func.instruction(&Instruction::LocalGet(self_ptr_local));
            func.instruction(&Instruction::I32WrapI64);
            emit_call(func, reloc_enabled, import_ids["obj_get_state"]);
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64LtS);
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(-1));
            func.instruction(&Instruction::I64Xor);
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::Else);
            if let Some(remap_entries) = state_remap_table_entries {
                let remap_base_local = state_remap_base_local
                    .expect("state remap base local missing for stateful wasm");
                let remap_value_local = state_remap_value_local
                    .expect("state remap value local missing for stateful wasm");
                func.instruction(&Instruction::LocalGet(state_local));
                func.instruction(&Instruction::I64Const(remap_entries));
                func.instruction(&Instruction::I64LtU);
                func.instruction(&Instruction::If(BlockType::Empty));
                func.instruction(&Instruction::LocalGet(remap_base_local));
                func.instruction(&Instruction::I32WrapI64);
                func.instruction(&Instruction::LocalGet(state_local));
                func.instruction(&Instruction::I32WrapI64);
                func.instruction(&Instruction::I32Const(8));
                func.instruction(&Instruction::I32Mul);
                func.instruction(&Instruction::I32Add);
                func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                    align: 3,
                    offset: 0,
                    memory_index: 0,
                }));
                func.instruction(&Instruction::LocalSet(remap_value_local));
                func.instruction(&Instruction::LocalGet(remap_value_local));
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64GeS);
                func.instruction(&Instruction::If(BlockType::Empty));
                func.instruction(&Instruction::LocalGet(remap_value_local));
                func.instruction(&Instruction::LocalSet(state_local));
                func.instruction(&Instruction::End);
                func.instruction(&Instruction::End);
            } else {
                emit_sparse_state_remap_lookup(
                    func,
                    state_local,
                    sparse_state_remap_entries
                        .as_deref()
                        .expect("sparse state remap entries missing for stateful wasm"),
                );
            }
            func.instruction(&Instruction::End);

            let dispatch_depths: Vec<u32> = (0..block_count)
                .map(|idx| (block_count - 1 - idx) as u32)
                .collect();

            let return_local = return_local.expect("stateful/jumpful missing return local");
            func.instruction(&Instruction::Block(BlockType::Empty));
            func.instruction(&Instruction::Loop(BlockType::Empty));
            for _ in (0..block_count).rev() {
                func.instruction(&Instruction::Block(BlockType::Empty));
            }

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(op_count as i64));
            func.instruction(&Instruction::I64GeU);
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::I64Const(block_count as i64));
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(block_map_base_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I32Const(4));
            func.instruction(&Instruction::I32Mul);
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                align: 2,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::I64ExtendI32U);
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::End);

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            let targets: Vec<u32> = (0..block_count).map(|idx| idx as u32).collect();
            func.instruction(&Instruction::BrTable(targets.into(), block_count as u32));
            func.instruction(&Instruction::End);

            let mut scratch_control: Vec<ControlKind> = Vec::new();
            let mut scratch_try: Vec<usize> = Vec::new();

            for (block_idx, start) in block_starts.iter().enumerate() {
                let end = block_starts.get(block_idx + 1).copied().unwrap_or(op_count);
                let depth = dispatch_depths[block_idx];
                let mut block_terminated = false;

                for idx in *start..end {
                    let op = &func_ir.ops[idx];
                    match op.kind.as_str() {
                        "state_switch" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "aiter" => {
                            let args = op.args.as_ref().unwrap();
                            let iter = locals[&args[0]];
                            func.instruction(&Instruction::LocalGet(iter));
                            emit_call(func, reloc_enabled, import_ids["aiter"]);
                            func.instruction(&Instruction::LocalSet(
                                locals[op.out.as_ref().unwrap()],
                            ));
                        }
                        "state_transition" => {
                            let args = op.args.as_ref().unwrap();
                            let future = locals[&args[0]];
                            let (slot_bits, pending_state) = if args.len() == 2 {
                                (None, locals[&args[1]])
                            } else {
                                (Some(locals[&args[1]]), locals[&args[2]])
                            };
                            let pending_state_name =
                                if args.len() == 2 { &args[1] } else { &args[2] };
                            let pending_target_idx = const_ints
                                .get(pending_state_name)
                                .and_then(|state_id| state_map.get(state_id).copied())
                                .map(|idx| !(idx as i64));
                            let next_state_id = op.value.unwrap();
                            let out = locals[op.out.as_ref().unwrap()];
                            let next_block = idx + 1;
                            let return_depth = depth + 2;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            if let Some(pending_encoded) = pending_target_idx {
                                func.instruction(&Instruction::I64Const(pending_encoded));
                            } else {
                                func.instruction(&Instruction::LocalGet(pending_state));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                            }
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::LocalGet(future));
                            emit_call(func, reloc_enabled, import_ids["future_poll"]);
                            func.instruction(&Instruction::LocalSet(out));
                            // Store pending return value before the
                            // conditional so the If block does not
                            // leave values on the stack.
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::LocalSet(return_local));
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::I64Eq);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::LocalGet(future));
                            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                            emit_call(func, reloc_enabled, import_ids["sleep_register"]);
                            func.instruction(&Instruction::Drop);
                            func.instruction(&Instruction::Br(return_depth));
                            func.instruction(&Instruction::End);
                            if let Some(slot) = slot_bits {
                                func.instruction(&Instruction::LocalGet(self_ptr_local));
                                func.instruction(&Instruction::I32WrapI64);
                                func.instruction(&Instruction::LocalGet(slot));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                                func.instruction(&Instruction::LocalGet(out));
                                emit_call(func, reloc_enabled, import_ids["closure_store"]);
                                func.instruction(&Instruction::Drop);
                            }
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::I64Const(next_state_id));
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "state_yield" => {
                            let args = op.args.as_ref().unwrap();
                            let pair = locals[&args[0]];
                            let resume_state_id = op.value.unwrap();
                            let resume_encoded = state_map
                                .get(&resume_state_id)
                                .copied()
                                .map(|idx| !(idx as i64));
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            if let Some(encoded) = resume_encoded {
                                func.instruction(&Instruction::I64Const(encoded));
                            } else {
                                func.instruction(&Instruction::I64Const(resume_state_id));
                            }
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::LocalGet(pair));
                            emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                            func.instruction(&Instruction::LocalGet(pair));
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        "chan_send_yield" => {
                            let args = op.args.as_ref().unwrap();
                            let chan = locals[&args[0]];
                            let val = locals[&args[1]];
                            let pending_state = locals[&args[2]];
                            let pending_state_name = &args[2];
                            let pending_target_idx = const_ints
                                .get(pending_state_name)
                                .and_then(|state_id| state_map.get(state_id).copied())
                                .map(|idx| !(idx as i64));
                            let next_state_id = op.value.unwrap();
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            if let Some(pending_encoded) = pending_target_idx {
                                func.instruction(&Instruction::I64Const(pending_encoded));
                            } else {
                                func.instruction(&Instruction::LocalGet(pending_state));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                            }
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::LocalGet(chan));
                            func.instruction(&Instruction::LocalGet(val));
                            emit_call(func, reloc_enabled, import_ids["chan_send"]);
                            let out = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalSet(out));
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::I64Eq);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::Return);
                            func.instruction(&Instruction::End);
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::I64Const(next_state_id));
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "chan_recv_yield" => {
                            let args = op.args.as_ref().unwrap();
                            let chan = locals[&args[0]];
                            let pending_state = locals[&args[1]];
                            let pending_state_name = &args[1];
                            let pending_target_idx = const_ints
                                .get(pending_state_name)
                                .and_then(|state_id| state_map.get(state_id).copied())
                                .map(|idx| !(idx as i64));
                            let next_state_id = op.value.unwrap();
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            if let Some(pending_encoded) = pending_target_idx {
                                func.instruction(&Instruction::I64Const(pending_encoded));
                            } else {
                                func.instruction(&Instruction::LocalGet(pending_state));
                                func.instruction(&Instruction::I64Const(INT_MASK as i64));
                                func.instruction(&Instruction::I64And);
                            }
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::LocalGet(chan));
                            emit_call(func, reloc_enabled, import_ids["chan_recv"]);
                            let out = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalSet(out));
                            func.instruction(&Instruction::LocalGet(out));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::I64Eq);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(box_pending()));
                            func.instruction(&Instruction::Return);
                            func.instruction(&Instruction::End);
                            func.instruction(&Instruction::LocalGet(self_ptr_local));
                            func.instruction(&Instruction::I32WrapI64);
                            func.instruction(&Instruction::I64Const(next_state_id));
                            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "if" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let else_idx = else_for_if.get(&idx).copied();
                            let end_idx = end_for_if.get(&idx).copied().unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "if without end_if")
                            });
                            let false_target = if let Some(else_pos) = else_idx {
                                else_pos + 1
                            } else {
                                end_idx + 1
                            };
                            let true_block = idx + 1;
                            let false_block = false_target;
                            let truthy_import = if wasm_scalar_truthiness_fast_path_for_name(
                                &scalar_plan,
                                &args[0],
                            ) {
                                "is_truthy_int"
                            } else {
                                "is_truthy"
                            };
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids[truthy_import],
                                reloc_enabled,
                            );
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(true_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(false_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "else" => {
                            let end_idx = end_for_else.get(&idx).copied().unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "else without end_if")
                            });
                            let end_block = end_idx + 1;
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "end_if" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_start" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_index_start" => {
                            let args = op.args.as_ref().unwrap();
                            let start = locals[&args[0]];
                            let out = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalGet(start));
                            func.instruction(&Instruction::LocalSet(out));
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_break_if_true" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break_if_true without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids["is_truthy"],
                                reloc_enabled,
                            );
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break_if_exception" => {
                            // Value-less exception-flag break in the jumpful
                            // state-machine lowering.  Mirrors `loop_break_if_true`
                            // but reads the sacrosanct `exception_pending` flag
                            // (`!= 0`) instead of an is_truthy(cond) value: TRUE
                            // (pending) -> jump to the loop-end state, FALSE ->
                            // fall through to the next state.
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break_if_exception without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break_if_false" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break_if_false without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids["is_truthy"],
                                reloc_enabled,
                            );
                            // Break when the condition is *falsy*: invert truthiness.
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break" => {
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_continue" => {
                            let start_idx =
                                loop_continue_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_continue without loop",
                                    )
                                });
                            let start_block = start_idx + 1;
                            func.instruction(&Instruction::I64Const(start_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_end" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "jump" => {
                            let target_label = op.value.expect("jump missing label");
                            let target_idx = label_to_index
                                .get(&target_label)
                                .copied()
                                .unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        format_args!("unknown jump label {target_label}"),
                                    )
                                });
                            let target_block = target_idx;
                            func.instruction(&Instruction::I64Const(target_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "br_if" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let target_label = op.value.unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "br_if missing label")
                            });
                            let target_idx = label_to_index
                                .get(&target_label)
                                .copied()
                                .unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        format_args!("unknown br_if label {target_label}"),
                                    )
                                });
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids["is_truthy"],
                                reloc_enabled,
                            );
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(target_idx as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                        }
                        "try_start" | "try_end" | "label" | "state_label" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "check_exception" => {
                            if native_eh_enabled {
                                // Native EH: skip polling; fall through to next state.
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                            } else if exception_handler_region_indices.contains(&idx) {
                                // Exception-handler regions operate on the currently
                                // pending exception. Re-polling here would immediately
                                // re-branch back into the same handler before
                                // exception_clear/print/cleanup can run.
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                            } else {
                                let target_label = op.value.unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "check_exception missing label",
                                    )
                                });
                                let target_idx = label_to_index
                                    .get(&target_label)
                                    .copied()
                                    .unwrap_or_else(|| {
                                        dispatch_control_panic(
                                            &func_ir.name,
                                            idx,
                                            format_args!(
                                                "unknown check_exception label {target_label}"
                                            ),
                                        )
                                    });
                                let target_block = target_idx;
                                let next_block = idx + 1;
                                emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                                func.instruction(&Instruction::I64Const(0));
                                func.instruction(&Instruction::I64Ne);
                                func.instruction(&Instruction::If(BlockType::Empty));
                                func.instruction(&Instruction::I64Const(target_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth + 1));
                                func.instruction(&Instruction::Else);
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth + 1));
                                func.instruction(&Instruction::End);
                                block_terminated = true;
                            }
                        }
                        "ret" => {
                            let ret_local =
                                op.var.as_ref().and_then(|name| locals.get(name).copied());
                            if let Some(local_idx) = ret_local {
                                func.instruction(&Instruction::LocalGet(local_idx));
                            } else {
                                dispatch_control_panic(
                                    &func_ir.name,
                                    idx,
                                    format_args!("ret target local {:?} is not present", op.var),
                                );
                            }
                            // Defensive arena teardown: state-machine functions
                            // do not currently produce arena-eligible allocs
                            // (StateYield forces GlobalEscape), but symmetry
                            // matters if escape analysis ever loosens.
                            if let Some(arena_idx) = arena_local {
                                func.instruction(&Instruction::LocalGet(arena_idx));
                                emit_call(func, reloc_enabled, import_ids["arena_free"]);
                            }
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        "ret_void" => {
                            if let Some(arena_idx) = arena_local {
                                func.instruction(&Instruction::LocalGet(arena_idx));
                                emit_call(func, reloc_enabled, import_ids["arena_free"]);
                            }
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        _ => {
                            op_emitter.emit_ops(
                                func,
                                std::slice::from_ref(op),
                                &mut scratch_control,
                                &mut scratch_try,
                                &mut label_stack,
                                &mut label_depths,
                                idx,
                            );
                        }
                    }
                    if block_terminated {
                        break;
                    }
                }

                let next_state = end;
                if !block_terminated {
                    func.instruction(&Instruction::I64Const(next_state as i64));
                    func.instruction(&Instruction::LocalSet(state_local));
                }
                func.instruction(&Instruction::Br(depth));

                if block_idx + 1 < block_count {
                    func.instruction(&Instruction::End);
                }
            }

            func.instruction(&Instruction::Br(0));
            func.instruction(&Instruction::End);
            const_cache.emit_none(func);
            func.instruction(&Instruction::LocalSet(return_local));
            func.instruction(&Instruction::End);
            // Defensive arena teardown for the stateful trailing return.
            if let Some(arena_idx) = arena_local {
                func.instruction(&Instruction::LocalGet(arena_idx));
                emit_call(func, reloc_enabled, import_ids["arena_free"]);
            }
            func.instruction(&Instruction::LocalGet(return_local));
            func.instruction(&Instruction::Return);
            func.instruction(&Instruction::End);
        } else if jumpful {
            let func = &mut func;
            let state_local = state_local.expect("state local missing for jumpful wasm");
            let op_count = func_ir.ops.len();
            let (block_starts, _) = dispatch_blocks
                .as_ref()
                .expect("dispatch blocks missing for jumpful wasm");
            let block_count = block_starts.len();
            let block_map_base_local =
                block_map_base_local.expect("block map base local missing for jumpful wasm");
            let dispatch_control_maps = dispatch_control_maps
                .as_ref()
                .expect("dispatch control maps missing for jumpful wasm");
            let label_to_index = &dispatch_control_maps.label_to_index;
            let else_for_if = &dispatch_control_maps.else_for_if;
            let end_for_if = &dispatch_control_maps.end_for_if;
            let end_for_else = &dispatch_control_maps.end_for_else;
            let loop_continue_target = &dispatch_control_maps.loop_continue_target;
            let loop_break_target = &dispatch_control_maps.loop_break_target;
            let exception_handler_region_indices: std::collections::BTreeSet<usize> = {
                let mut regions = std::collections::BTreeSet::new();
                let handler_labels: Vec<i64> = func_ir
                    .ops
                    .iter()
                    .filter_map(|op| (op.kind == "check_exception").then_some(op.value).flatten())
                    .collect();
                for label in handler_labels {
                    let Some(&start_idx) = label_to_index.get(&label) else {
                        continue;
                    };
                    let mut nested_pushes = 0usize;
                    for handler_idx in start_idx..op_count {
                        let handler_op = &func_ir.ops[handler_idx];
                        regions.insert(handler_idx);
                        match handler_op.kind.as_str() {
                            "exception_push" => nested_pushes += 1,
                            "exception_pop" => {
                                if nested_pushes == 0 {
                                    break;
                                }
                                nested_pushes -= 1;
                            }
                            "ret" | "ret_void" => break,
                            _ => {}
                        }
                    }
                }
                regions
            };

            let mut scratch_control: Vec<ControlKind> = Vec::new();
            let mut scratch_try: Vec<usize> = Vec::new();
            let mut label_stack: Vec<i64> = Vec::new();
            let mut label_depths: BTreeMap<i64, usize> = BTreeMap::new();

            let dispatch_depths: Vec<u32> = (0..block_count)
                .map(|idx| (block_count - 1 - idx) as u32)
                .collect();

            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::LocalSet(state_local));

            func.instruction(&Instruction::Loop(BlockType::Empty));
            for _ in (0..block_count).rev() {
                func.instruction(&Instruction::Block(BlockType::Empty));
            }

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I64Const(op_count as i64));
            func.instruction(&Instruction::I64GeU);
            func.instruction(&Instruction::If(BlockType::Empty));
            func.instruction(&Instruction::I64Const(block_count as i64));
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::LocalGet(block_map_base_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I32Const(4));
            func.instruction(&Instruction::I32Mul);
            func.instruction(&Instruction::I32Add);
            func.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                align: 2,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::I64ExtendI32U);
            func.instruction(&Instruction::LocalSet(state_local));
            func.instruction(&Instruction::End);

            func.instruction(&Instruction::LocalGet(state_local));
            func.instruction(&Instruction::I32WrapI64);
            let targets: Vec<u32> = (0..block_count).map(|idx| idx as u32).collect();
            func.instruction(&Instruction::BrTable(targets.into(), block_count as u32));
            func.instruction(&Instruction::End);

            for (block_idx, start) in block_starts.iter().enumerate() {
                let end = block_starts.get(block_idx + 1).copied().unwrap_or(op_count);
                let depth = dispatch_depths[block_idx];
                let mut block_terminated = false;

                for idx in *start..end {
                    let op = &func_ir.ops[idx];
                    match op.kind.as_str() {
                        "state_switch" | "state_transition" | "state_yield" | "chan_send_yield"
                        | "chan_recv_yield" => {
                            dispatch_control_panic(
                                &func_ir.name,
                                idx,
                                format_args!("jumpful path hit stateful op {}", op.kind),
                            );
                        }
                        "if" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let else_idx = else_for_if.get(&idx).copied();
                            let end_idx = end_for_if.get(&idx).copied().unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "if without end_if")
                            });
                            let false_target = if let Some(else_pos) = else_idx {
                                else_pos + 1
                            } else {
                                end_idx + 1
                            };
                            let true_block = idx + 1;
                            let false_block = false_target;
                            let truthy_import = if wasm_scalar_truthiness_fast_path_for_name(
                                &scalar_plan,
                                &args[0],
                            ) {
                                "is_truthy_int"
                            } else {
                                "is_truthy"
                            };
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids[truthy_import],
                                reloc_enabled,
                            );
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(true_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(false_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "else" => {
                            let end_idx = end_for_else.get(&idx).copied().unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "else without end_if")
                            });
                            let end_block = end_idx + 1;
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "end_if" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_start" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_index_start" => {
                            let args = op.args.as_ref().unwrap();
                            let start = locals[&args[0]];
                            let out = locals[op.out.as_ref().unwrap()];
                            func.instruction(&Instruction::LocalGet(start));
                            func.instruction(&Instruction::LocalSet(out));
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_break_if_true" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break_if_true without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids["is_truthy"],
                                reloc_enabled,
                            );
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break_if_exception" => {
                            // Value-less exception-flag break in the jumpful
                            // state-machine lowering.  Mirrors `loop_break_if_true`
                            // but reads the sacrosanct `exception_pending` flag
                            // (`!= 0`) instead of an is_truthy(cond) value: TRUE
                            // (pending) -> jump to the loop-end state, FALSE ->
                            // fall through to the next state.
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break_if_exception without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::I64Ne);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break_if_false" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break_if_false without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            let next_block = idx + 1;
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids["is_truthy"],
                                reloc_enabled,
                            );
                            // Break when the condition is *falsy*: invert truthiness.
                            func.instruction(&Instruction::I32Eqz);
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::Else);
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                            block_terminated = true;
                        }
                        "loop_break" => {
                            let end_idx =
                                loop_break_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_break without loop",
                                    )
                                });
                            let end_block = end_idx + 1;
                            func.instruction(&Instruction::I64Const(end_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_continue" => {
                            let start_idx =
                                loop_continue_target.get(&idx).copied().unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "loop_continue without loop",
                                    )
                                });
                            let start_block = start_idx + 1;
                            func.instruction(&Instruction::I64Const(start_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "loop_end" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "jump" => {
                            let target_label = op.value.unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "jump missing label")
                            });
                            let target_idx = label_to_index
                                .get(&target_label)
                                .copied()
                                .unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        format_args!("unknown jump label {target_label}"),
                                    )
                                });
                            let target_block = target_idx;
                            func.instruction(&Instruction::I64Const(target_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "br_if" => {
                            let args = op.args.as_ref().unwrap();
                            let cond = locals[&args[0]];
                            let target_label = op.value.unwrap_or_else(|| {
                                dispatch_control_panic(&func_ir.name, idx, "br_if missing label")
                            });
                            let target_idx = label_to_index
                                .get(&target_label)
                                .copied()
                                .unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        format_args!("unknown br_if label {target_label}"),
                                    )
                                });
                            emit_branch_truthiness_i32(
                                func,
                                cond,
                                import_ids["is_truthy"],
                                reloc_enabled,
                            );
                            func.instruction(&Instruction::If(BlockType::Empty));
                            func.instruction(&Instruction::I64Const(target_idx as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth + 1));
                            func.instruction(&Instruction::End);
                        }
                        "try_start" | "try_end" | "label" | "state_label" => {
                            let next_block = idx + 1;
                            func.instruction(&Instruction::I64Const(next_block as i64));
                            func.instruction(&Instruction::LocalSet(state_local));
                            func.instruction(&Instruction::Br(depth));
                            block_terminated = true;
                        }
                        "check_exception" => {
                            if native_eh_enabled {
                                // Native EH: skip polling; fall through to next state.
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                            } else if exception_handler_region_indices.contains(&idx) {
                                let next_block = idx + 1;
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth));
                                block_terminated = true;
                            } else {
                                let target_label = op.value.unwrap_or_else(|| {
                                    dispatch_control_panic(
                                        &func_ir.name,
                                        idx,
                                        "check_exception missing label",
                                    )
                                });
                                let target_idx = label_to_index
                                    .get(&target_label)
                                    .copied()
                                    .unwrap_or_else(|| {
                                        dispatch_control_panic(
                                            &func_ir.name,
                                            idx,
                                            format_args!(
                                                "unknown check_exception label {target_label}"
                                            ),
                                        )
                                    });
                                let target_block = target_idx;
                                let next_block = idx + 1;
                                emit_call(func, reloc_enabled, import_ids["exception_pending"]);
                                func.instruction(&Instruction::I64Const(0));
                                func.instruction(&Instruction::I64Ne);
                                func.instruction(&Instruction::If(BlockType::Empty));
                                func.instruction(&Instruction::I64Const(target_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth + 1));
                                func.instruction(&Instruction::Else);
                                func.instruction(&Instruction::I64Const(next_block as i64));
                                func.instruction(&Instruction::LocalSet(state_local));
                                func.instruction(&Instruction::Br(depth + 1));
                                func.instruction(&Instruction::End);
                                block_terminated = true;
                            }
                        }
                        "ret" => {
                            let ret_local =
                                op.var.as_ref().and_then(|name| locals.get(name).copied());
                            if let Some(local_idx) = ret_local {
                                func.instruction(&Instruction::LocalGet(local_idx));
                            } else {
                                dispatch_control_panic(
                                    &func_ir.name,
                                    idx,
                                    format_args!("ret target local {:?} is not present", op.var),
                                );
                            }
                            // Defensive arena teardown: state-machine functions
                            // do not currently produce arena-eligible allocs
                            // (StateYield forces GlobalEscape), but symmetry
                            // matters if escape analysis ever loosens.
                            if let Some(arena_idx) = arena_local {
                                func.instruction(&Instruction::LocalGet(arena_idx));
                                emit_call(func, reloc_enabled, import_ids["arena_free"]);
                            }
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        "ret_void" => {
                            if let Some(arena_idx) = arena_local {
                                func.instruction(&Instruction::LocalGet(arena_idx));
                                emit_call(func, reloc_enabled, import_ids["arena_free"]);
                            }
                            func.instruction(&Instruction::I64Const(0));
                            func.instruction(&Instruction::Return);
                            block_terminated = true;
                        }
                        _ => {
                            op_emitter.emit_ops(
                                func,
                                std::slice::from_ref(op),
                                &mut scratch_control,
                                &mut scratch_try,
                                &mut label_stack,
                                &mut label_depths,
                                idx,
                            );
                        }
                    }
                    if block_terminated {
                        break;
                    }
                }

                let next_state = end;
                if !block_terminated {
                    func.instruction(&Instruction::I64Const(next_state as i64));
                    func.instruction(&Instruction::LocalSet(state_local));
                }
                func.instruction(&Instruction::Br(depth));

                if block_idx + 1 < block_count {
                    func.instruction(&Instruction::End);
                }
            }
            func.instruction(&Instruction::Br(0));
            func.instruction(&Instruction::End);
            // Defensive arena teardown for the stateful trailing return.
            if let Some(arena_idx) = arena_local {
                func.instruction(&Instruction::LocalGet(arena_idx));
                emit_call(func, reloc_enabled, import_ids["arena_free"]);
            }
            const_cache.emit_none(func);
            func.instruction(&Instruction::Return);
            func.instruction(&Instruction::End);
        } else {
            let func = &mut func;
            let mut jump_labels: BTreeSet<i64> = BTreeSet::new();
            let mut label_order: Vec<i64> = Vec::new();
            for op in &func_ir.ops {
                match op.kind.as_str() {
                    "jump" => {
                        if let Some(label_id) = op.value {
                            jump_labels.insert(label_id);
                        }
                    }
                    "label" => {
                        if let Some(label_id) = op.value {
                            label_order.push(label_id);
                        }
                    }
                    _ => {}
                }
            }
            let label_ids: Vec<i64> = label_order
                .into_iter()
                .filter(|label_id| jump_labels.contains(label_id))
                .collect();
            if !label_ids.is_empty() {
                for label_id in label_ids.iter().rev() {
                    func.instruction(&Instruction::Block(BlockType::Empty));
                    control_stack.push(ControlKind::Block);
                    label_depths.insert(*label_id, control_stack.len() - 1);
                    label_stack.push(*label_id);
                }
            }
            op_emitter.emit_ops(
                func,
                &func_ir.ops,
                &mut control_stack,
                &mut try_stack,
                &mut label_stack,
                &mut label_depths,
                0,
            );
            while !label_stack.is_empty() {
                label_stack.pop();
                func.instruction(&Instruction::End);
                control_stack.pop();
            }
            // Plain functions can legally rely on Python's implicit `None`
            // return. Match the stateful/jumpful lowering paths instead of
            // falling off the end of an i64-returning WASM function.
            // Free the per-function ScopeArena before falling off the end —
            // explicit `ret` ops free their own arena, but implicit-`None`
            // fallthrough still needs the symmetric teardown.
            if let Some(arena_idx) = arena_local {
                func.instruction(&Instruction::LocalGet(arena_idx));
                emit_call(func, reloc_enabled, import_ids["arena_free"]);
            }
            const_cache.emit_none(func);
            func.instruction(&Instruction::End);
        }

        drop(op_emitter);

        // Accumulate tail call count from this function into the backend total.
        self.tail_calls_emitted += tail_call_count.get();

        self.codes.function(&func);
    }
}
