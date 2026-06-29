use super::*;

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) struct FunctionPreanalysis {
    pub(in crate::native_backend::function_compiler) has_ret: bool,
    pub(in crate::native_backend::function_compiler) stateful: bool,
    pub(in crate::native_backend::function_compiler) has_store: bool,
    pub(in crate::native_backend::function_compiler) var_names: Vec<String>,
    pub(in crate::native_backend::function_compiler) last_use: BTreeMap<String, usize>,
    pub(in crate::native_backend::function_compiler) alias_roots: BTreeMap<String, String>,
    pub(in crate::native_backend::function_compiler) if_to_end_if: BTreeMap<usize, usize>,
    pub(in crate::native_backend::function_compiler) if_to_else: BTreeMap<usize, usize>,
    pub(in crate::native_backend::function_compiler) else_to_end_if: BTreeMap<usize, usize>,
    pub(in crate::native_backend::function_compiler) label_ids: Vec<i64>,
    pub(in crate::native_backend::function_compiler) state_label_ids: BTreeSet<i64>,
    pub(in crate::native_backend::function_compiler) shared_resume_label_ids: BTreeSet<i64>,
    pub(in crate::native_backend::function_compiler) state_ids: Vec<i64>,
    pub(in crate::native_backend::function_compiler) resume_states: BTreeSet<i64>,
    pub(in crate::native_backend::function_compiler) function_exception_label_id: Option<i64>,
    pub(in crate::native_backend::function_compiler) exception_label_ids: BTreeSet<i64>,
    /// Pre-built map from variable name -> constant integer value for O(1) lookups.
    /// Only the first definition of each name is stored (SSA correctness).
    pub(in crate::native_backend::function_compiler) const_int_map: BTreeMap<String, i64>,
    /// Variables assigned (op.out) inside each loop body, keyed by the
    /// loop_start / loop_index_start op index.  Used to emit per-iteration
    /// dec_ref at the loop back-edge so reassigned containers are freed
    /// instead of leaking.
    pub(in crate::native_backend::function_compiler) loop_body_out_vars:
        BTreeMap<usize, Vec<String>>,
    /// Subset of loop_body_out_vars that lack any reaching pre-loop store.
    /// These need an explicit None sentinel before the first iteration so
    /// the native backend has a valid old-value slot for loop-carried cleanup.
    pub(in crate::native_backend::function_compiler) loop_body_init_vars:
        BTreeMap<usize, Vec<String>>,
    /// True when any op in this function is marked `arena_eligible`.
    /// Triggers scope-arena lifecycle (molt_arena_new at entry,
    /// molt_arena_alloc for eligible allocs, molt_arena_free at exit).
    pub(in crate::native_backend::function_compiler) has_arena_eligible: bool,
    /// Set of output variable names from arena-eligible alloc ops.
    pub(in crate::native_backend::function_compiler) arena_eligible_outs: BTreeSet<String>,
    /// Scalar-like variables (int/bool/float) that MUST stay slot-backed
    /// because they escape the local scalar fast-path scope.  A variable
    /// is unsafe to exclude when ANY of:
    ///   - it is passed as an argument to a function call
    ///   - it is stored to the heap (store_attr, store_index on non-inline containers)
    ///   - it is returned from the function (ret)
    ///   - it has explicit inc_ref/dec_ref ops in the IR
    pub(in crate::native_backend::function_compiler) scalar_slot_exclusion_unsafe: BTreeSet<String>,
    /// Per-field-store ownership facts for fresh fixed-layout object payloads.
    /// `FreshInit` means the old slot is proven uninitialized zero storage,
    /// even when the surface op is `store`; `DirectNonHeap` is the narrower
    /// performance fact that both old and new slot contents are non-heap.
    pub(in crate::native_backend::function_compiler) field_store_modes:
        BTreeMap<usize, FieldStoreMode>,
    /// RC drop-insertion substrate (design 20, R1 guard): true when the TIR
    /// drop-insertion pass processed this function (detected via the leading
    /// `drop_inserted` marker op). When set, the ad-hoc `loop_reassign_old_val`
    /// per-iteration dec-ref path is DISABLED — the TIR pass already inserted the
    /// loop-carried DecRef, and running both would double-drop (refcount
    /// underflow → use-after-free / abort).
    pub(in crate::native_backend::function_compiler) drop_inserted: bool,
}

#[cfg(feature = "native-backend")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(in crate::native_backend::function_compiler) enum FieldStoreMode {
    FreshInit,
    DirectNonHeap,
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn import_func_ref(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder,
    local_refs: &mut BTreeMap<&'static str, FuncRef>,
    name: &'static str,
    params: &[types::Type],
    returns: &[types::Type],
) -> FuncRef {
    let func_id = SimpleBackend::import_func_id_split(module, import_ids, name, params, returns);
    if let Some(func_ref) = local_refs.get(name) {
        return *func_ref;
    }
    let func_ref = module.declare_func_in_func(func_id, builder.func);
    local_refs.insert(name, func_ref);
    func_ref
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn import_runtime_func_ref(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder,
    local_refs: &mut BTreeMap<&'static str, FuncRef>,
    signature: crate::runtime_import_abi::RuntimeImportSignature,
) -> FuncRef {
    let func_id = SimpleBackend::import_runtime_func_id_split(module, import_ids, signature);
    if let Some(func_ref) = local_refs.get(signature.name) {
        return *func_ref;
    }
    let func_ref = module.declare_func_in_func(func_id, builder.func);
    local_refs.insert(signature.name, func_ref);
    func_ref
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn declare_function_object_target(
    module: &mut ObjectModule,
    op_kind: &str,
    func_name: &str,
    linkage: Linkage,
    sig: &cranelift_codegen::ir::Signature,
) -> cranelift_module::FuncId {
    let expected_params = sig.params.len();
    let returns_value = !sig.returns.is_empty();
    module
        .declare_function(func_name, linkage, sig)
        .unwrap_or_else(|err| {
            panic!(
                "{op_kind} declaration mismatch for `{func_name}`: expected \
                 {expected_params} parameter(s), returns={returns_value}: {err}"
            )
        })
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn require_static_target_symbol(
    op: &OpIR,
) -> &str {
    op.s_value
        .as_deref()
        .unwrap_or_else(|| panic!("{} missing static target symbol", op.kind))
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn emit_guarded_object_field_get(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    obj_bits: Value,
    offset_bytes: i64,
    nbc: &crate::NanBoxConsts,
) -> Value {
    let tag_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
    let tag_bits = builder.ins().band(obj_bits, tag_mask);
    let ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
    let is_ptr = builder.ins().icmp(IntCC::Equal, tag_bits, ptr_tag);
    let none_bits = builder.ins().iconst(types::I64, box_none());

    let load_block = builder.create_block();
    let none_block = builder.create_block();
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, types::I64);
    if let Some(current_block) = builder.current_block() {
        builder.insert_block_after(load_block, current_block);
        builder.insert_block_after(none_block, load_block);
        builder.insert_block_after(merge_block, none_block);
    }
    builder.ins().brif(is_ptr, load_block, &[], none_block, &[]);

    switch_to_block_materialized(builder, load_block);
    seal_block_once(builder, sealed_blocks, load_block);
    let obj_ptr = unbox_ptr_value(builder, obj_bits, nbc);
    let offset = builder.ins().iconst(types::I64, offset_bytes);
    let callee = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        "molt_object_field_get_ptr",
        &[types::I64, types::I64],
        &[types::I64],
    );
    let call = builder.ins().call(callee, &[obj_ptr, offset]);
    let result = builder.inst_results(call)[0];
    jump_block(builder, merge_block, &[result]);

    switch_to_block_materialized(builder, none_block);
    seal_block_once(builder, sealed_blocks, none_block);
    jump_block(builder, merge_block, &[none_bits]);

    switch_to_block_materialized(builder, merge_block);
    seal_block_once(builder, sealed_blocks, merge_block);
    builder.block_params(merge_block)[0]
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn preanalyze_alias_source<'a>(
    op: &'a OpIR,
    return_alias_summaries: &BTreeMap<String, crate::passes::ReturnAliasSummary>,
) -> Option<&'a str> {
    match op.kind.as_str() {
        "copy" => op.var.as_deref().or_else(|| {
            op.args
                .as_ref()
                .and_then(|args| args.first())
                .map(String::as_str)
        }),
        "copy_var" | "load_var" => op
            .args
            .as_ref()
            .and_then(|args| args.first())
            .map(String::as_str)
            .or(op.var.as_deref()),
        "box" | "unbox" | "cast" | "widen" | "identity_alias" | "store_var" => op
            .args
            .as_ref()
            .and_then(|args| args.first())
            .map(String::as_str),
        "call" => {
            let callee = op.s_value.as_ref()?;
            let crate::passes::ReturnAliasSummary::Param(param_idx) =
                *return_alias_summaries.get(callee)?;
            op.args
                .as_ref()
                .and_then(|args| args.get(param_idx))
                .map(String::as_str)
        }
        _ => None,
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn simple_ir_op_absorbs_finalizer_elements(
    op: &OpIR,
) -> bool {
    matches!(
        op.kind.as_str(),
        "build_list" | "build_tuple" | "build_dict" | "build_set"
    ) || crate::tir::op_kinds_generated::kind_result_absorbs_operand_ownership_table(&op.kind)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn preanalysis_value_is_known_non_heap(
    name: &str,
    representation_plan: &ScalarRepresentationPlan,
) -> bool {
    representation_plan.name_is_non_heap_scalar(name)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn direct_field_store_control_boundary(
    kind: &str,
) -> bool {
    matches!(
        kind,
        "label"
            | "state_label"
            | "jump"
            | "br_if"
            | "if"
            | "else"
            | "end_if"
            | "loop_start"
            | "loop_end"
            | "loop_break_if_true"
            | "loop_break_if_false"
            | "loop_break_if_exception"
            | "loop_break"
            | "loop_continue"
            | "ret"
            | "ret_void"
    )
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn direct_field_store_passthrough(
    kind: &str,
) -> bool {
    matches!(
        kind,
        "copy"
            | "copy_var"
            | "load_var"
            | "store_var"
            | "delete_var"
            | "identity_alias"
            | "const"
            | "const_bool"
            | "const_float"
            | "const_none"
            | "const_str"
            | "const_bytes"
            | "line"
            | "nop"
            | "trace_enter_slot"
            | "trace_exit"
            | "missing"
    )
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn remove_direct_field_store_root(
    root: &str,
    direct_object_roots: &mut BTreeSet<String>,
    known_non_heap_slots: &mut BTreeSet<(String, i64)>,
) {
    direct_object_roots.remove(root);
    known_non_heap_slots.retain(|(slot_root, _)| slot_root != root);
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn op_allocates_fresh_fixed_layout_object(
    op: &OpIR,
) -> bool {
    match op.kind.as_str() {
        "object_new_bound_stack" => op.value.is_some_and(|payload_size| payload_size > 0),
        "object_new_bound" => op.value.is_some_and(|payload_size| payload_size > 0),
        _ => false,
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn analyze_field_store_modes(
    func_ir: &FunctionIR,
    alias_roots: &BTreeMap<String, String>,
    representation_plan: &ScalarRepresentationPlan,
) -> BTreeMap<usize, FieldStoreMode> {
    let mut modes = BTreeMap::new();
    let mut direct_object_roots: BTreeSet<String> = BTreeSet::new();
    let mut initialized_slots: BTreeSet<(String, i64)> = BTreeSet::new();
    let mut known_non_heap_slots: BTreeSet<(String, i64)> = BTreeSet::new();

    for (idx, op) in func_ir.ops.iter().enumerate() {
        let kind = op.kind.as_str();
        if direct_field_store_control_boundary(kind) {
            direct_object_roots.clear();
            initialized_slots.clear();
            known_non_heap_slots.clear();
            continue;
        }

        if op_allocates_fresh_fixed_layout_object(op) {
            if let Some(out) = op.out.as_deref() {
                let root = alias_root_name(alias_roots, out).to_string();
                direct_object_roots.insert(root);
            }
            continue;
        }

        if matches!(kind, "store" | "store_init") {
            let Some(args) = op.args.as_ref() else {
                continue;
            };
            let (Some(obj_name), Some(value_name)) = (args.first(), args.get(1)) else {
                continue;
            };
            let root = alias_root_name(alias_roots, obj_name).to_string();
            if !direct_object_roots.contains(&root) {
                continue;
            }
            let offset = op.value.unwrap_or(0);
            let value_known_non_heap =
                preanalysis_value_is_known_non_heap(value_name, representation_plan);
            let slot = (root.clone(), offset);
            let slot_initialized = initialized_slots.contains(&slot);
            let slot_known_non_heap = known_non_heap_slots.contains(&slot);

            if kind == "store_init" || !slot_initialized {
                modes.insert(idx, FieldStoreMode::FreshInit);
                initialized_slots.insert(slot.clone());
                if value_known_non_heap {
                    known_non_heap_slots.insert(slot);
                } else {
                    known_non_heap_slots.remove(&slot);
                }
                continue;
            }

            if value_known_non_heap && slot_known_non_heap {
                modes.insert(idx, FieldStoreMode::DirectNonHeap);
                initialized_slots.insert(slot.clone());
                known_non_heap_slots.insert(slot);
            } else {
                initialized_slots.insert(slot.clone());
                known_non_heap_slots.remove(&slot);
            }
            continue;
        }

        if direct_field_store_passthrough(kind) {
            continue;
        }

        let mut touched_roots: BTreeSet<String> = BTreeSet::new();
        if let Some(args) = op.args.as_ref() {
            for arg in args {
                let root = alias_root_name(alias_roots, arg).to_string();
                if direct_object_roots.contains(&root) {
                    touched_roots.insert(root);
                }
            }
        }
        if let Some(var) = op.var.as_ref() {
            let root = alias_root_name(alias_roots, var).to_string();
            if direct_object_roots.contains(&root) {
                touched_roots.insert(root);
            }
        }
        for root in touched_roots {
            remove_direct_field_store_root(
                &root,
                &mut direct_object_roots,
                &mut known_non_heap_slots,
            );
            initialized_slots.retain(|(slot_root, _)| slot_root != &root);
        }
    }

    modes
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn preanalyze_function_ir(
    func_ir: &FunctionIR,
    return_alias_summaries: &BTreeMap<String, crate::passes::ReturnAliasSummary>,
    representation_plan: &ScalarRepresentationPlan,
) -> FunctionPreanalysis {
    let mut has_ret = false;
    let mut stateful = false;
    // RC drop-insertion substrate (design 20, R1 guard): set by the leading
    // `drop_inserted` marker op the TIR back-conversion emits for drop-processed
    // functions.
    let mut drop_inserted = false;
    let mut var_names: BTreeSet<String> = BTreeSet::new();
    let mut last_use = BTreeMap::new();
    let mut alias_roots = BTreeMap::new();
    let mut if_to_end_if = BTreeMap::new();
    let mut if_to_else = BTreeMap::new();
    let mut else_to_end_if = BTreeMap::new();
    let mut if_stack: Vec<(usize, Option<usize>)> = Vec::new();
    let mut state_ids = Vec::new();
    let mut seen_state_ids: BTreeSet<i64> = BTreeSet::new();
    let mut label_ids = Vec::new();
    let mut seen_label_ids: BTreeSet<i64> = BTreeSet::new();
    let mut state_label_ids = BTreeSet::new();
    let mut resume_states = BTreeSet::new();
    let mut exception_label_ids = BTreeSet::new();
    let mut label_positions = Vec::new();
    let const_int_map = crate::build_const_int_map(&func_ir.ops);
    for name in &func_ir.params {
        if name != "none" {
            var_names.insert(name.clone());
            alias_roots.insert(name.clone(), name.clone());
        }
    }

    for (idx, op) in func_ir.ops.iter().enumerate() {
        match op.kind.as_str() {
            "ret" => has_ret = true,
            "drop_inserted" => drop_inserted = true,
            "state_switch" | "state_transition" | "state_yield" | "chan_send_yield"
            | "chan_recv_yield" => stateful = true,
            "store" => {}
            _ => {}
        }

        let logical_out = op.out.as_ref().or_else(|| {
            op.var
                .as_ref()
                .filter(|_| matches!(op.kind.as_str(), "store_var" | "delete_var"))
        });
        if let Some(out) = logical_out
            && out != "none"
        {
            var_names.insert(out.clone());
            // Seed outputs with their definition site so unused temporaries
            // can still be released deterministically after this op.
            last_use.entry(out.clone()).or_insert(idx);
            if let Some(src) = preanalyze_alias_source(op, return_alias_summaries) {
                let root = alias_roots
                    .get(src)
                    .cloned()
                    .unwrap_or_else(|| src.to_string());
                alias_roots.insert(out.clone(), root);
            } else {
                alias_roots
                    .entry(out.clone())
                    .or_insert_with(|| out.clone());
            }
            let heap_literal_uses_data_segment =
                fc::const_literals::op_uses_heap_literal_data_segment(op);
            if heap_literal_uses_data_segment {
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
                    if matches!(op.kind.as_str(), "label" | "state_label")
                        && seen_label_ids.insert(state_id)
                    {
                        label_ids.push(state_id);
                    }
                    if matches!(
                        op.kind.as_str(),
                        "state_transition"
                            | "state_yield"
                            | "chan_send_yield"
                            | "chan_recv_yield"
                            | "state_label"
                    ) {
                        resume_states.insert(state_id);
                    }
                    if op.kind == "state_label" {
                        state_label_ids.insert(state_id);
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
    let mut loop_body_out_vars: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut loop_body_init_vars: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    // Per-iteration heap temporaries of a generator/async `_poll` (see the
    // computation below). Declared in the function scope — assigned exactly once
    // inside the unconditional block below — so the later alias-group last_use
    // unification can also keep them un-extended.
    let stateful_per_iter_temps: BTreeSet<String>;
    {
        let mut loop_stack_post: Vec<usize> = Vec::new(); // stack of loop start indices
        let mut loop_ranges: Vec<(usize, usize)> = Vec::new();
        for (idx, op) in func_ir.ops.iter().enumerate() {
            match op.kind.as_str() {
                "loop_start" => {
                    // Indexed loops may materialize loop-invariant constants
                    // between LOOP_START and LOOP_INDEX_START. Treat that
                    // whole prelude as part of the indexed-loop opener so we
                    // do not push a duplicate plain loop frame.
                    let indexed_follows = loop_start_has_index_prelude(&func_ir.ops, idx);
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

        // Post-pass 2: detect back-edge loops for TIR-generated control flow.
        // When TIR linearizes loops into label/jump/br_if, store_var ops
        // inside these loops need explicit inc_ref/dec_ref (the store_var
        // handler checks `back_edge_ranges` to decide).  Computed BEFORE the
        // func_end lifetime extension below so the per-iteration-temporary
        // analysis can see both structured and TIR-linearized loop bodies.
        let back_edge_ranges: Vec<(usize, usize)> = {
            let mut ranges = Vec::new();
            let mut label_pos: std::collections::HashMap<i64, usize> =
                std::collections::HashMap::new();
            for (idx, op) in func_ir.ops.iter().enumerate() {
                if matches!(op.kind.as_str(), "label" | "state_label")
                    && let Some(id) = op.value
                {
                    label_pos.insert(id, idx);
                }
            }
            for (idx, op) in func_ir.ops.iter().enumerate() {
                if matches!(op.kind.as_str(), "jump" | "br_if")
                    && let Some(target_id) = op.value
                    && let Some(&target_pos) = label_pos.get(&target_id)
                    && target_pos < idx
                {
                    ranges.push((target_pos, idx));
                }
            }
            ranges
        };

        // ── Shared per-iteration-dead analysis inputs ──
        //
        // Computed unconditionally (cheap linear scans) so BOTH the generator/async
        // `_poll` per-iteration analysis (`stateful_per_iter_temps`) AND the
        // ExceptionRegion creation-ref analysis. Each is a single pass over
        // `func_ir.ops`.
        //
        // First definition site of every name (min index over defining ops).
        let mut first_def: BTreeMap<&str, usize> = BTreeMap::new();
        for name in &func_ir.params {
            if name != "none" {
                first_def.entry(name.as_str()).or_insert(0);
            }
        }
        for (idx, op) in func_ir.ops.iter().enumerate() {
            if let Some(out) = op.out.as_deref()
                && out != "none"
            {
                first_def.entry(out).or_insert(idx);
            }
            // Local slot mutations logically (re)define their destination variable.
            if matches!(op.kind.as_str(), "store_var" | "delete_var")
                && let Some(var) = op.var.as_deref()
                && var != "none"
            {
                first_def.entry(var).or_insert(idx);
            }
        }
        // Names that are ever a local-slot mutation target carry loop/handler state in a
        // slot (they are slot-backed and balanced by the store_var retain-new/
        // release-old path); never treat them as per-iteration temps. For a stored
        // exception `saved = e`, the slot `saved` is the store TARGET (and stays
        // func_end-extended with its own independent reference), while the exception
        // op RESULT is the store SOURCE — so the result still qualifies and
        // releasing it at its last use cannot free the stored object.
        let mut store_var_targets: BTreeSet<&str> = BTreeSet::new();
        for op in &func_ir.ops {
            if matches!(op.kind.as_str(), "store_var" | "delete_var")
                && let Some(name) = op.var.as_deref().or(op.out.as_deref())
                && name != "none"
            {
                store_var_targets.insert(name);
            }
        }
        // Linear indices of every suspend op (yield / await / channel rendezvous).
        // A value whose live range *strictly contains* a suspend must survive the
        // poll's return-and-resume, so it is never a per-iteration temporary.
        // (A `try`/`except` function with no suspend ops yields an empty list, so
        // the suspend test is vacuously satisfied for the exception analyses.)
        let suspend_ops: Vec<usize> = func_ir
            .ops
            .iter()
            .enumerate()
            .filter(|(_, op)| {
                matches!(
                    op.kind.as_str(),
                    "state_yield" | "state_transition" | "chan_send_yield" | "chan_recv_yield"
                )
            })
            .map(|(idx, _)| idx)
            .collect();

        // A name N is a per-iteration temporary — releasable at its real last use
        // by the ordinary in-body / suspend-boundary / control-flow drain rather
        // than deferred to func_end — iff ALL of:
        //
        //   1. N is NOT loop-carried: no back-edge body (s, e) has
        //      `first_def(N) < s <= last_use(N)`.  That predicate means N is defined
        //      before a loop header `s` and still read at/after it, so it must
        //      survive the back-edge.  Its negation guarantees N's live range does
        //      not straddle any loop header — N is recomputed each iteration (a
        //      fresh SSA temporary), not threaded around the loop.  This admits both
        //      in-body temporaries (the `(value, done)` pair built right before a
        //      `state_yield`) AND resume-prologue temporaries (the `yield from`
        //      delegation pair from `iter_next`, defined before the loop header
        //      and dead before it).
        //
        //   2. No suspend op lies STRICTLY INSIDE `(first_def(N), last_use(N))`.
        //      If a yield/await sat between N's definition and its last read, N would
        //      have to survive the poll's return; the open interval lets a value
        //      whose last use IS the suspend (the yielded pair) still qualify — it is
        //      released by the suspend-boundary drain.  Vacuous for a non-stateful
        //      `try`/`except` function.
        //
        //   3. N is not a `store_var` target — those carry loop/handler state in a
        //      slot and are balanced by the store_var retain-new/release-old path.
        //
        // `last` is the global maximum use index and `first_def` the global minimum
        // definition index, so these interval tests bound EVERY reference to N.
        let is_per_iter_dead = |name: &str, last: usize| -> bool {
            if name == "none" || store_var_targets.contains(name) {
                return false;
            }
            let Some(&def) = first_def.get(name) else {
                return false;
            };
            if last < def {
                return false;
            }
            if back_edge_ranges.iter().any(|&(s, _e)| def < s && s <= last) {
                return false; // loop-carried
            }
            if suspend_ops.iter().any(|&sx| def < sx && sx < last) {
                return false; // live range strictly contains a suspend
            }
            true
        };

        // ── Per-iteration temporaries in generator/async `_poll` state machines ──
        //
        // The blanket "extend every lifetime to func_end" model below implements
        // the Swift-ARC release-at-scope-exit discipline: a loop-carried heap value
        // is released once, at the function's return, instead of inside the loop.
        // For an ORDINARY function that is correct — the return *is* the scope exit,
        // reached once after the loop completes.
        //
        // A generator/async `_poll` is a state machine that RETURNS ON EVERY YIELD
        // and is re-entered on the next resume.  Its "function return" is a yield
        // SUSPENSION, not the generator's scope exit.  Extending a *per-iteration*
        // heap temporary's lifetime to func_end therefore defers its release to a
        // point that, on the suspend path, never drains it — so the temporary is
        // re-allocated and orphaned on every resume.  The canonical victim is the
        // `(value, done)` pair tuple built by `tuple_new` immediately before each
        // `state_yield`: it is allocated rc=1, retained to rc=2 by the suspend
        // (so it survives the return to the consumer), and the consumer releases it
        // once → rc=1, leaked.  Over a streamed generator (and multiplied by every
        // delegation level of `yield from` / `for y in inner(): yield …`) this is an
        // unbounded O(iterations × depth) leak.
        //
        // Fix: in a `stateful` function, do NOT extend the lifetime of values that
        // are genuinely dead within a single iteration.  Their real `last_use` is
        // preserved so the suspend-boundary drain (added in the `state_yield` /
        // `state_transition` / `chan_*_yield` handlers) releases them per-iteration —
        // byte-identical semantics, O(active-chain-depth) memory.  Loop-carried
        // values (accumulators, cell-list contents) are live across the back-edge,
        // so they fail the "dead within one iteration" test and remain fully
        // protected by the func_end extension.
        stateful_per_iter_temps = if stateful && !drop_inserted {
            last_use
                .iter()
                .filter(|(name, last)| is_per_iter_dead(name.as_str(), **last))
                .map(|(name, _)| name.clone())
                .collect()
        } else {
            BTreeSet::new()
        };

        // Extend ALL variable lifetimes to function end for ANY function
        // that has loops (structured or TIR-generated). This prevents
        // drain_cleanup_tracked from emitting premature dec_ref for values
        // stored in cell lists during loop iterations.
        //
        // Why func_end and not loop_end: drain_cleanup_tracked fires at
        // multiple intermediate points (check_exception, label transitions,
        // store_index calls). If any of these is AFTER the loop_end index
        // but BEFORE the function return, the dec_ref frees cell list values
        // that are still referenced by the cell list.
        //
        // This is the Swift ARC pattern: retain at store, release at scope
        // exit (function return). The only cost is delayed cleanup.
        //
        // RC drop-insertion substrate (design 20 §4.1, Phase 5): this func-end
        // lifetime extension exists SOLELY to keep `drain_cleanup_tracked_*` from
        // emitting a premature dec_ref on a loop-carried value (the Swift-ARC
        // release-at-scope-exit model). When the TIR drop pass owns this
        // function's RC, the value-tracking drains are already neutralized (the
        // registration skip above leaves the tracked lists empty), and the TIR
        // `DecRef(old)` on the back-edge releases the previous iteration's value
        // precisely. Extending every variable's lifetime to func_end would defeat
        // that precision, so it is dropped for drop-inserted functions. (SSA
        // block-param threading for loop-carried values is handled by the TIR phi
        // / native join-slot machinery, not by this RC-only extension.)
        //
        // `stateful_per_iter_temps` are excluded: their release belongs INSIDE the
        // loop body (at the suspend boundary), not at the per-yield return — see the
        // generator-`_poll` analysis above.
        if !loop_ranges.is_empty() && !drop_inserted {
            let func_end = func_ir.ops.len().saturating_sub(1);
            for (name, entry) in last_use.iter_mut() {
                if stateful_per_iter_temps.contains(name) {
                    continue;
                }
                if *entry < func_end {
                    *entry = func_end;
                }
            }
        }
        // Also extend last_use for variables in back-edge ranges to the
        // back-edge jump position. This prevents drain_cleanup_tracked
        // from emitting ADDITIONAL dec_ref beyond what store_var handles.
        // For back-edge loops: extend ALL variables to function end.
        // This is the only approach that prevents all premature dec_ref.
        // Memory leak is bounded by function scope (cleanup at ret).
        //
        // RC drop-insertion substrate (design 20 §4.1, Phase 5): same rationale
        // as the structured-loop extension above — this is an RC-tracking-only
        // lifetime extension. Drop it for drop-inserted functions, whose RC is
        // owned by the TIR `DecRef`/`IncRef` ops (the back-edge `DecRef(old)`
        // releases the carried value with per-iteration precision).
        //
        // `stateful_per_iter_temps` excluded for the same reason as the
        // structured-loop extension: a generator/async `_poll`'s per-iteration
        // heap temporaries are released at the suspend boundary, not deferred to
        // the per-yield return (which would orphan them on every resume).
        if !back_edge_ranges.is_empty() && !drop_inserted {
            let func_end = func_ir.ops.len().saturating_sub(1);
            for (name, entry) in last_use.iter_mut() {
                if stateful_per_iter_temps.contains(name) {
                    continue;
                }
                if *entry < func_end {
                    *entry = func_end;
                }
            }
        }

        // Finalizer-sensitive named locals (#58 native value-tracking lane).
        //
        // The native value-tracking substrate must honor the same lifetime fact
        // as TIR drop insertion: a finalizer-bearing object bound to a Python
        // local lives until that binding is deleted/rebound/scope-exited, while
        // an unnamed expression temporary may die at statement last use.
        if !drop_inserted {
            let root_of = |name: &str| -> String {
                alias_roots
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| name.to_string())
            };
            let mut finalizer_sensitive: BTreeSet<String> = BTreeSet::new();
            for op in &func_ir.ops {
                if op.defines_del == Some(true)
                    && let Some(out) = op.out.as_deref()
                    && out != "none"
                {
                    finalizer_sensitive.insert(root_of(out));
                }
            }
            let mut changed = true;
            while changed {
                changed = false;
                for op in &func_ir.ops {
                    if !simple_ir_op_absorbs_finalizer_elements(op) {
                        continue;
                    }
                    let absorbs_sensitive = op.args.as_ref().is_some_and(|args| {
                        args.iter()
                            .any(|arg| finalizer_sensitive.contains(&root_of(arg)))
                    });
                    if absorbs_sensitive
                        && let Some(out) = op.out.as_deref()
                        && out != "none"
                        && finalizer_sensitive.insert(root_of(out))
                    {
                        changed = true;
                    }
                }
            }
            if !finalizer_sensitive.is_empty() {
                let explicit_del_roots: BTreeSet<String> = func_ir
                    .ops
                    .iter()
                    .filter(|op| matches!(op.kind.as_str(), "del_boundary" | "delete_var"))
                    .filter_map(|op| op.args.as_ref())
                    .flat_map(|args| args.iter())
                    .filter(|name| name.as_str() != "none")
                    .map(|name| root_of(name))
                    .collect();
                let func_end = func_ir.ops.len().saturating_sub(1);
                let mut named_roots: BTreeSet<String> = BTreeSet::new();
                for op in &func_ir.ops {
                    if op.bound_local != Some(true) {
                        continue;
                    }
                    let Some(out) = op.out.as_deref() else {
                        continue;
                    };
                    if out == "none" {
                        continue;
                    }
                    let root = root_of(out);
                    if finalizer_sensitive.contains(&root) && !explicit_del_roots.contains(&root) {
                        named_roots.insert(root);
                    }
                }
                for root in named_roots {
                    let entry = last_use.entry(root).or_insert(func_end);
                    if *entry < func_end {
                        *entry = func_end;
                    }
                }
            }
        }

        // Collect loop-carried slot assignments inside each loop body.
        // Only named storage slots (`store_var`) need CPython-style
        // "old slot occupant" handling across iterations. SSA temporaries are
        // recomputed within an iteration and must not be forced into
        // loop-carried state, or check_exception fallthrough can select stale
        // previous-iteration values for transient heap objects.
        for &(start, end) in &loop_ranges {
            let mut assigned: Vec<String> = Vec::new();
            let mut init_needed: Vec<String> = Vec::new();
            let mut seen: BTreeSet<String> = BTreeSet::new();
            // Identify the loop counter name so we can exclude it —
            // the loop machinery manages its refcount separately.
            let counter_name: Option<&str> = {
                let start_op = &func_ir.ops[start];
                if start_op.kind == "loop_index_start" {
                    start_op.out.as_deref()
                } else {
                    // For plain loop_start, scan forward for loop_index_start
                    let mut cn = None;
                    for idx in (start + 1)..end {
                        if func_ir.ops[idx].kind == "loop_index_start" {
                            cn = func_ir.ops[idx].out.as_deref();
                            break;
                        }
                        if !func_ir.ops[idx].kind.starts_with("const") {
                            break;
                        }
                    }
                    cn
                }
            };
            for idx in (start + 1)..end {
                let op = &func_ir.ops[idx];
                if !matches!(op.kind.as_str(), "store_var" | "delete_var") {
                    continue;
                }
                if let Some(name) = &op.var
                    && name != "none"
                    && counter_name != Some(name.as_str())
                    && is_persistent_local_slot_name(name)
                    && seen.insert(name.clone())
                {
                    assigned.push(name.clone());
                    let has_pre_loop_store = func_ir.ops[..start].iter().any(|prior| {
                        matches!(prior.kind.as_str(), "store_var" | "delete_var")
                            && prior.var.as_deref() == Some(name.as_str())
                    });
                    if !has_pre_loop_store {
                        init_needed.push(name.clone());
                    }
                }
            }
            if !assigned.is_empty() {
                loop_body_out_vars.insert(start, assigned);
            }
            if !init_needed.is_empty() {
                loop_body_init_vars.insert(start, init_needed);
            }
        }
    }

    // Post-pass: alias-equivalent SSA names must share the latest use.
    // Helper wrappers commonly return an input unchanged, and direct-call
    // alias summaries propagate that identity into the caller. If we only
    // track textual uses per SSA name, cleanup sites such as
    // `check_exception` can decref an earlier alias before a later alias
    // reaches the function return.
    {
        let mut max_last_use_by_root: BTreeMap<String, usize> = BTreeMap::new();
        for (name, root) in &alias_roots {
            let Some(last) = last_use.get(name).copied() else {
                continue;
            };
            max_last_use_by_root
                .entry(root.clone())
                .and_modify(|slot| {
                    if *slot < last {
                        *slot = last;
                    }
                })
                .or_insert(last);
        }
        for (name, root) in &alias_roots {
            // A per-iteration `_poll` temporary must keep its real last use so the
            // suspend-boundary drain releases it each iteration; do not let the
            // alias-group unification re-extend it to a group-mate's later use.
            if stateful_per_iter_temps.contains(name) {
                continue;
            }
            let Some(group_last) = max_last_use_by_root.get(root).copied() else {
                continue;
            };
            let entry = last_use.entry(name.clone()).or_insert(group_last);
            if *entry < group_last {
                *entry = group_last;
            }
        }
    }

    let field_store_modes = analyze_field_store_modes(func_ir, &alias_roots, representation_plan);
    let has_store = func_ir.ops.iter().enumerate().any(|(idx, op)| {
        op.kind == "store" && field_store_modes.get(&idx) != Some(&FieldStoreMode::DirectNonHeap)
    });

    let mut var_names: Vec<String> = var_names.into_iter().collect();
    var_names.sort();
    let function_exception_label_id = label_positions
        .into_iter()
        .rev()
        .find_map(|(_, id)| exception_label_ids.contains(&id).then_some(id));

    let label_id_set: BTreeSet<i64> = label_ids.iter().copied().collect();
    let mut shared_resume_label_ids = state_label_ids.clone();
    for op in &func_ir.ops {
        let pending_arg = match op.kind.as_str() {
            "state_transition" => {
                let Some(args) = op.args.as_ref() else {
                    continue;
                };
                match args.as_slice() {
                    [_, pending_state] => Some(pending_state),
                    [_, _, pending_state] => Some(pending_state),
                    _ => None,
                }
            }
            "chan_send_yield" => op.args.as_ref().and_then(|args| args.get(2)),
            "chan_recv_yield" => op.args.as_ref().and_then(|args| args.get(1)),
            _ => None,
        };
        let Some(pending_arg) = pending_arg else {
            continue;
        };
        let Some(&pending_state_id) = const_int_map.get(pending_arg) else {
            continue;
        };
        resume_states.insert(pending_state_id);
        assert!(
            label_id_set.contains(&pending_state_id),
            "function {} stores pending resume state {} from {} but has no matching label/state_label",
            func_ir.name,
            pending_state_id,
            op.kind.as_str(),
        );
        shared_resume_label_ids.insert(pending_state_id);
    }

    // Scope arena eligibility: detect alloc ops marked arena_eligible.
    let mut has_arena_eligible = false;
    let mut arena_eligible_outs: BTreeSet<String> = BTreeSet::new();
    for op in &func_ir.ops {
        if op.arena_eligible == Some(true) {
            has_arena_eligible = true;
            if let Some(ref out) = op.out {
                arena_eligible_outs.insert(out.clone());
            }
        }
    }

    let scalar_slot_exclusion_unsafe = representation_plan.scalar_slot_exclusion_unsafe();

    FunctionPreanalysis {
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
        state_label_ids,
        shared_resume_label_ids,
        state_ids,
        resume_states,
        function_exception_label_id,
        exception_label_ids,
        const_int_map,
        loop_body_out_vars,
        loop_body_init_vars,
        has_arena_eligible,
        arena_eligible_outs,
        scalar_slot_exclusion_unsafe,
        field_store_modes,
        drop_inserted,
    }
}
