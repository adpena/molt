use super::constant_ops::{
    const_seed_bits, needs_literal_pointer_locals, needs_seeded_runtime_const,
};
use super::context::CompileFuncContext;
use super::control_flow::has_non_linear_control_flow;
use super::*;

pub(super) fn collect_read_vars(ops: &[OpIR]) -> BTreeSet<String> {
    let mut read_vars = BTreeSet::new();
    for op in ops {
        if let Some(args) = &op.args {
            for arg in args {
                read_vars.insert(arg.clone());
            }
        }
        if let Some(var) = &op.var {
            read_vars.insert(var.clone());
        }
    }
    read_vars
}

pub(super) struct WasmLocalLayout {
    pub(super) locals: BTreeMap<String, u32>,
    pub(super) local_types: Vec<ValType>,
    pub(super) runtime_lookup_only_vars: BTreeSet<String>,
    pub(super) scalar_plan: ScalarRepresentationPlan,
    pub(super) stateful: bool,
    pub(super) jumpful: bool,
    pub(super) tail_call_eligible: bool,
    pub(super) arena_local: Option<u32>,
    pub(super) self_ptr_local: Option<u32>,
    pub(super) state_local: Option<u32>,
    pub(super) block_map_base_local: Option<u32>,
    pub(super) return_local: Option<u32>,
    pub(super) state_remap_base_local: Option<u32>,
    pub(super) state_remap_value_local: Option<u32>,
    pub(super) const_cache: ConstantCache,
    pub(super) const_seed_locals: Vec<(u32, i64)>,
    pub(super) seeded_runtime_const_ops: Vec<(usize, OpIR)>,
    pub(super) seeded_runtime_const_op_indices: BTreeSet<usize>,
    pub(super) is_multi_return_callee: Option<usize>,
    pub(super) multi_ret_locals: Vec<u32>,
    pub(super) multi_ret_tuple_vars: BTreeSet<String>,
    pub(super) multi_ret_call_locals: BTreeMap<(String, i64), u32>,
    pub(super) multi_ret_call_vars: BTreeSet<String>,
}

impl WasmLocalLayout {
    pub(super) fn for_function(func_ir: &FunctionIR, ctx: &CompileFuncContext<'_>) -> Self {
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
        // ever *read* (appear in op.args or op.var). Output-only variables
        // that are never read can share a single WASM local ("dead sink"),
        // reducing the total local count and binary size.
        let read_vars = collect_read_vars(&func_ir.ops);
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
                if needs_literal_pointer_locals(&op.kind) {
                    // _ptr and _len locals are used internally by the op
                    // emission so they always need real (non-sink) locals.
                    ensure_local_inner(&format!("{out}_ptr"), false);
                    ensure_local_inner(&format!("{out}_len"), false);
                }
                if !const_seed_seen.contains(out) {
                    let bits = const_seed_bits(op);
                    if let Some(bits) = bits {
                        // Skip seeding dead locals -- the value is never
                        // observed so there is no point initializing it.
                        if !is_dead {
                            const_seed_seen.insert(out.clone());
                            const_seed_locals_all.push((out_local_idx, bits));
                        }
                    } else if needs_seeded_runtime_const(&op.kind) && !is_dead {
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
        // QNAN_TAG_PTR_I64 into locals unconditionally â€” these large i64
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

        // --- Tail call optimization eligibility (WASM tail calls proposal Â§3.5) ---
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
        Self {
            locals,
            local_types,
            runtime_lookup_only_vars,
            scalar_plan,
            stateful,
            jumpful,
            tail_call_eligible,
            arena_local,
            self_ptr_local,
            state_local,
            block_map_base_local,
            return_local,
            state_remap_base_local,
            state_remap_value_local,
            const_cache,
            const_seed_locals,
            seeded_runtime_const_ops,
            seeded_runtime_const_op_indices,
            is_multi_return_callee,
            multi_ret_locals,
            multi_ret_tuple_vars,
            multi_ret_call_locals,
            multi_ret_call_vars,
        }
    }
}
