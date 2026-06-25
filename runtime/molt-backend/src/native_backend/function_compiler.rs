use super::*;
use crate::repr::{ContainerKind, ContainerStorageKind, ScalarKind};
use crate::representation_plan::ScalarRepresentationPlan;

// Per-op-family Cranelift codegen handlers lifted out of `compile_func_inner`
// (decomposition program M1). Scalar carrier/boxing helpers live in
// `scalar_carriers.rs` and stay visible only inside `function_compiler`, so the
// extracted `fc::*` families do not widen backend APIs.
mod scalar_carriers;
use scalar_carriers::*;
mod fc;
use fc::list_index_fast_path::{
    ListIndexFastPathState, loop_start_has_index_prelude, metadata_only_structured_loop_ops,
};

#[cfg(feature = "native-backend")]
static EMPTY_VEC_STRING: Vec<String> = Vec::new();

#[cfg(feature = "native-backend")]
#[inline]
fn is_cold_module_chunk_function(name: &str) -> bool {
    name.contains("__molt_module_chunk_")
}

#[cfg(feature = "native-backend")]
fn collect_slot_backed_join_names(
    ops: &[OpIR],
    exception_label_ids: &BTreeSet<i64>,
    stateful: bool,
) -> BTreeSet<String> {
    let mut slot_backed_join_names: BTreeSet<String> = BTreeSet::new();

    // Join carriers that are explicitly materialized with store/load in the IR
    // are memory-backed transport by construction. Keep them on the stack-backed
    // path so later label materialization does not try to reinterpret them as
    // structured phi joins.
    for op in ops {
        if matches!(op.kind.as_str(), "store_var" | "delete_var")
            && let Some(name) = op.var.as_ref().or(op.out.as_ref())
            && is_join_slot_name(name)
        {
            slot_backed_join_names.insert(name.clone());
        }
    }

    // Stateful functions (generators / async / comprehension polls) carry their
    // SSA values across state_yield / state_label resume points the same way
    // exception-bearing functions carry values across check_exception splits.
    // The state machine generates many block edges that aren't eagerly sealed,
    // so phi resolution at seal_all_blocks() time can explode block-parameter
    // counts past regalloc2's u32-indexed entity tables (u32::MAX panic).
    //
    // Treat stateful functions like exception functions: route all store_var
    // targets through stack slots so the state machine carries memory values,
    // not SSA values, across resume edges.
    if exception_label_ids.is_empty() && !stateful {
        return slot_backed_join_names;
    }

    let mut exception_region_depth = 0i32;
    let mut first_seen_join_in_exception: BTreeMap<String, bool> = BTreeMap::new();
    let mut exception_written_locals: BTreeSet<String> = BTreeSet::new();

    // Collect ALL persistent local-slot mutation targets that appear anywhere
    // in a function with exception handling or stateful resume points. When the function defers
    // block sealing to seal_all_blocks(), Cranelift must resolve SSA phi
    // nodes for every variable that has definitions reaching from different
    // predecessors. Each check_exception or state_yield creates a new block
    // split, and variables carried across these splits become block
    // parameters. In functions with many such splits (e.g. try/except
    // bodies, generator/async poll state machines), the block parameter
    // count explodes and can overflow regalloc2's internal index tables
    // (u32::MAX index panic).
    //
    // By routing all persistent local storage through stack slots instead of SSA
    // variables, we eliminate the phi nodes entirely. Stack loads/stores
    // are slightly slower than register-to-register moves, but:
    // 1. Exception-handling and poll functions are already on the cold path
    // 2. The alternative is a hard backend compile failure
    // 3. regalloc2 phi resolution for many-predecessor blocks is O(n^2)
    //
    // This is the same strategy used by LLVM's mem2reg in the presence of
    // exception handling: keep values in memory across EH boundaries.
    let mut all_store_var_targets: BTreeSet<String> = BTreeSet::new();
    for op in ops {
        if matches!(op.kind.as_str(), "store_var" | "delete_var")
            && let Some(name) = op.var.as_ref().or(op.out.as_ref())
            && is_persistent_local_slot_name(name)
        {
            all_store_var_targets.insert(name.clone());
        }
    }
    // All persistent store_var targets in exception-bearing or stateful functions
    // use stack slots. Compiler SSA temps remain SSA values; they are not Python
    // local storage and widening them to stack slots can erase representation
    // facts at check_exception boundaries.
    slot_backed_join_names.extend(all_store_var_targets);

    for op in ops {
        match op.kind.as_str() {
            "try_start" => {
                exception_region_depth += 1;
            }
            "exception_pop" => {
                exception_region_depth = (exception_region_depth - 1).max(0);
            }
            "store_var" | "delete_var" if exception_region_depth > 0 => {
                if let Some(name) = op.var.as_ref().or(op.out.as_ref())
                    && is_persistent_local_slot_name(name)
                {
                    exception_written_locals.insert(name.clone());
                    if is_join_slot_name(name) {
                        first_seen_join_in_exception
                            .entry(name.clone())
                            .or_insert(true);
                    }
                }
            }
            "copy_var" | "load_var" if exception_region_depth > 0 => {
                let candidate = op
                    .var
                    .as_ref()
                    .or_else(|| op.args.as_ref().and_then(|args| args.first()));
                if let Some(name) = candidate
                    && is_join_slot_name(name)
                {
                    first_seen_join_in_exception
                        .entry(name.clone())
                        .or_insert(true);
                }
            }
            _ => {}
        }
    }
    for (name, in_exception) in first_seen_join_in_exception {
        if in_exception {
            slot_backed_join_names.insert(name);
        }
    }
    slot_backed_join_names.extend(exception_written_locals);
    slot_backed_join_names
}

#[cfg(feature = "native-backend")]
#[cfg(test)]
fn live_exception_rebind_vars_for_op(
    vars: &BTreeMap<String, Variable>,
    transport_last_use: &BTreeMap<String, usize>,
    first_defined_at: &BTreeMap<String, usize>,
    op_idx: usize,
) -> BTreeMap<String, Variable> {
    vars.iter()
        .filter_map(|(name, var)| {
            let last = transport_last_use.get(name).copied().unwrap_or(usize::MAX);
            let has_reaching_def = first_defined_at
                .get(name)
                .copied()
                .is_some_and(|first| first <= op_idx);
            (has_reaching_def && last > op_idx).then_some((name.clone(), *var))
        })
        .collect()
}

#[cfg(feature = "native-backend")]
fn switch_to_block_with_rebind(
    builder: &mut FunctionBuilder,
    block: Block,
    is_block_filled: &mut bool,
    _has_exception_labels: bool,
) {
    crate::switch_to_block_tracking(builder, block, is_block_filled);
    // Do not synthesize implicit SSA transport here.
    //
    // Cranelift materializes missing `use_var` state at a block switch by
    // appending block params. That is only correct when the predecessor edges
    // explicitly transport those values. Merge payloads and exception
    // fallthrough state must therefore be modeled by real block params or
    // slot-backed joins at the call site, not by opportunistic rebinding here.
}

#[cfg(feature = "native-backend")]
fn materialize_label_block(
    builder: &mut FunctionBuilder,
    block: Block,
    is_block_filled: &mut bool,
) {
    ensure_block_in_layout(builder, block);
    // If we're already inside `block` and it's still open, the label has
    // effectively materialised in place — do not emit a self-jump to itself,
    // which would (a) close the block, (b) wire it as its own predecessor,
    // and (c) generate an unreachable trailing instruction. The
    // `is_block_filled` guard alone is not sufficient because a fresh
    // resume block created by `state_yield` lowering also has
    // `is_block_filled == false` while already being the current block.
    let already_in_target = builder.current_block() == Some(block);
    if !already_in_target {
        if !*is_block_filled {
            jump_block(builder, block, &[]);
        }
        crate::switch_to_block_tracking(builder, block, is_block_filled);
    }
}

#[cfg(feature = "native-backend")]
#[inline]
fn switch_to_block_materialized(builder: &mut FunctionBuilder, block: Block) {
    ensure_block_in_layout(builder, block);
    builder.switch_to_block(block);
}

#[cfg(feature = "native-backend")]
struct FunctionPreanalysis {
    has_ret: bool,
    stateful: bool,
    has_store: bool,
    var_names: Vec<String>,
    last_use: BTreeMap<String, usize>,
    alias_roots: BTreeMap<String, String>,
    if_to_end_if: BTreeMap<usize, usize>,
    if_to_else: BTreeMap<usize, usize>,
    else_to_end_if: BTreeMap<usize, usize>,
    label_ids: Vec<i64>,
    state_label_ids: BTreeSet<i64>,
    shared_resume_label_ids: BTreeSet<i64>,
    state_ids: Vec<i64>,
    resume_states: BTreeSet<i64>,
    function_exception_label_id: Option<i64>,
    exception_label_ids: BTreeSet<i64>,
    /// Pre-built map from variable name -> constant integer value for O(1) lookups.
    /// Only the first definition of each name is stored (SSA correctness).
    const_int_map: BTreeMap<String, i64>,
    /// Variables assigned (op.out) inside each loop body, keyed by the
    /// loop_start / loop_index_start op index.  Used to emit per-iteration
    /// dec_ref at the loop back-edge so reassigned containers are freed
    /// instead of leaking.
    loop_body_out_vars: BTreeMap<usize, Vec<String>>,
    /// Subset of loop_body_out_vars that lack any reaching pre-loop store.
    /// These need an explicit None sentinel before the first iteration so
    /// the native backend has a valid old-value slot for loop-carried cleanup.
    loop_body_init_vars: BTreeMap<usize, Vec<String>>,
    int_like_vars: BTreeSet<String>,
    bool_like_vars: BTreeSet<String>,
    float_like_vars: BTreeSet<String>,
    str_like_vars: BTreeSet<String>,
    none_like_vars: BTreeSet<String>,
    /// True when any op in this function is marked `arena_eligible`.
    /// Triggers scope-arena lifecycle (molt_arena_new at entry,
    /// molt_arena_alloc for eligible allocs, molt_arena_free at exit).
    has_arena_eligible: bool,
    /// Set of output variable names from arena-eligible alloc ops.
    arena_eligible_outs: BTreeSet<String>,
    /// Scalar-like variables (int/bool/float) that MUST stay slot-backed
    /// because they escape the local scalar fast-path scope.  A variable
    /// is unsafe to exclude when ANY of:
    ///   - it is passed as an argument to a function call
    ///   - it is stored to the heap (store_attr, store_index on non-inline containers)
    ///   - it is returned from the function (ret)
    ///   - it has explicit inc_ref/dec_ref ops in the IR
    scalar_slot_exclusion_unsafe: BTreeSet<String>,
    /// Per-field-store ownership facts for fresh fixed-layout object payloads.
    /// `FreshInit` means the old slot is proven uninitialized zero storage,
    /// even when the surface op is `store`; `DirectNonHeap` is the narrower
    /// performance fact that both old and new slot contents are non-heap.
    field_store_modes: BTreeMap<usize, FieldStoreMode>,
    /// RC drop-insertion substrate (design 20, R1 guard): true when the TIR
    /// drop-insertion pass processed this function (detected via the leading
    /// `drop_inserted` marker op). When set, the ad-hoc `loop_reassign_old_val`
    /// per-iteration dec-ref path is DISABLED — the TIR pass already inserted the
    /// loop-carried DecRef, and running both would double-drop (refcount
    /// underflow → use-after-free / abort).
    drop_inserted: bool,
}

#[cfg(feature = "native-backend")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum FieldStoreMode {
    FreshInit,
    DirectNonHeap,
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
    let func_id = SimpleBackend::import_func_id_split(module, import_ids, name, params, returns);
    if let Some(func_ref) = local_refs.get(name) {
        return *func_ref;
    }
    let func_ref = module.declare_func_in_func(func_id, builder.func);
    local_refs.insert(name, func_ref);
    func_ref
}

#[cfg(feature = "native-backend")]
fn declare_function_object_target(
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
fn require_static_target_symbol(op: &OpIR) -> &str {
    op.s_value
        .as_deref()
        .unwrap_or_else(|| panic!("{} missing static target symbol", op.kind))
}

#[cfg(feature = "native-backend")]
fn emit_guarded_object_field_get(
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
fn preanalyze_alias_source<'a>(
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
fn simple_ir_op_absorbs_finalizer_elements(op: &OpIR) -> bool {
    matches!(
        op.kind.as_str(),
        "build_list" | "build_tuple" | "build_dict" | "build_set"
    ) || crate::tir::op_kinds_generated::kind_result_absorbs_operand_ownership_table(&op.kind)
}

#[cfg(feature = "native-backend")]
fn preanalysis_value_is_known_non_heap(
    name: &str,
    int_like_vars: &BTreeSet<String>,
    bool_like_vars: &BTreeSet<String>,
    float_like_vars: &BTreeSet<String>,
    none_like_vars: &BTreeSet<String>,
) -> bool {
    int_like_vars.contains(name)
        || bool_like_vars.contains(name)
        || float_like_vars.contains(name)
        || none_like_vars.contains(name)
}

#[cfg(feature = "native-backend")]
fn direct_field_store_control_boundary(kind: &str) -> bool {
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
fn direct_field_store_passthrough(kind: &str) -> bool {
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
fn remove_direct_field_store_root(
    root: &str,
    direct_object_roots: &mut BTreeSet<String>,
    known_non_heap_slots: &mut BTreeSet<(String, i64)>,
) {
    direct_object_roots.remove(root);
    known_non_heap_slots.retain(|(slot_root, _)| slot_root != root);
}

#[cfg(feature = "native-backend")]
fn op_allocates_fresh_fixed_layout_object(op: &OpIR) -> bool {
    match op.kind.as_str() {
        "object_new_bound_stack" => op.value.is_some_and(|payload_size| payload_size > 0),
        "object_new_bound" => op.value.is_some_and(|payload_size| payload_size > 0),
        _ => false,
    }
}

#[cfg(feature = "native-backend")]
fn analyze_field_store_modes(
    func_ir: &FunctionIR,
    alias_roots: &BTreeMap<String, String>,
    int_like_vars: &BTreeSet<String>,
    bool_like_vars: &BTreeSet<String>,
    float_like_vars: &BTreeSet<String>,
    none_like_vars: &BTreeSet<String>,
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
            let value_known_non_heap = preanalysis_value_is_known_non_heap(
                value_name,
                int_like_vars,
                bool_like_vars,
                float_like_vars,
                none_like_vars,
            );
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
fn preanalyze_function_ir(
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
    let (int_like_vars, bool_like_vars, float_like_vars, str_like_vars, none_like_vars) =
        representation_plan.scalar_name_sets();

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

    let field_store_modes = analyze_field_store_modes(
        func_ir,
        &alias_roots,
        &int_like_vars,
        &bool_like_vars,
        &float_like_vars,
        &none_like_vars,
    );
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
        int_like_vars,
        bool_like_vars,
        float_like_vars,
        str_like_vars,
        none_like_vars,
        has_arena_eligible,
        arena_eligible_outs,
        scalar_slot_exclusion_unsafe,
        field_store_modes,
        drop_inserted,
    }
}

#[cfg(feature = "native-backend")]
fn next_check_exception_target(ops: &[OpIR], op_idx: usize) -> Option<i64> {
    ops.iter()
        .skip(op_idx + 1)
        .find(|op| op.kind == "check_exception")
        .and_then(|op| op.value)
}

#[cfg(feature = "native-backend")]
fn remove_tracked_name(tracked: &mut Vec<String>, name: &str) {
    tracked.retain(|tracked_name| tracked_name != name);
}

#[cfg(feature = "native-backend")]
fn is_join_slot_name(name: &str) -> bool {
    name.starts_with("_bb") && name.contains("_arg")
}

#[cfg(feature = "native-backend")]
fn is_compiler_value_temp_name(name: &str) -> bool {
    name.strip_prefix("_v")
        .or_else(|| name.strip_prefix('v'))
        .is_some_and(|suffix| suffix.as_bytes().first().is_some_and(u8::is_ascii_digit))
}

#[cfg(feature = "native-backend")]
fn is_persistent_local_slot_name(name: &str) -> bool {
    is_join_slot_name(name) || !is_compiler_value_temp_name(name)
}

#[cfg(feature = "native-backend")]
fn remove_tracked_alias_group(
    tracked: &mut Vec<String>,
    alias_roots: &BTreeMap<String, String>,
    root: &str,
) {
    tracked.retain(|name| alias_roots.get(name).map(String::as_str) != Some(root));
}

#[cfg(feature = "native-backend")]
fn alias_root_name<'a>(alias_roots: &'a BTreeMap<String, String>, name: &'a str) -> &'a str {
    alias_roots.get(name).map(String::as_str).unwrap_or(name)
}

#[cfg(feature = "native-backend")]
fn cleanup_roots_for_names(
    alias_roots: &BTreeMap<String, String>,
    names: impl IntoIterator<Item = String>,
) -> BTreeSet<String> {
    names
        .into_iter()
        .map(|name| alias_root_name(alias_roots, &name).to_string())
        .collect()
}

#[cfg(feature = "native-backend")]
fn scrub_tracked_roots(
    roots: &BTreeSet<String>,
    alias_roots: &BTreeMap<String, String>,
    tracked_vars: &mut Vec<String>,
    tracked_obj_vars: &mut Vec<String>,
    tracked_vars_set: &mut std::collections::HashSet<String>,
    tracked_obj_vars_set: &mut std::collections::HashSet<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
) {
    if roots.is_empty() {
        return;
    }
    tracked_obj_vars.retain(|n: &String| !roots.contains(alias_root_name(alias_roots, n.as_str())));
    tracked_vars.retain(|n: &String| !roots.contains(alias_root_name(alias_roots, n.as_str())));
    tracked_obj_vars_set.retain(|n| !roots.contains(alias_root_name(alias_roots, n.as_str())));
    tracked_vars_set.retain(|n| !roots.contains(alias_root_name(alias_roots, n.as_str())));
    entry_vars.retain(|name, _| !roots.contains(alias_root_name(alias_roots, name)));
    for tracked_list in block_tracked_obj.values_mut() {
        tracked_list.retain(|name| !roots.contains(alias_root_name(alias_roots, name.as_str())));
    }
    for tracked_list in block_tracked_ptr.values_mut() {
        tracked_list.retain(|name| !roots.contains(alias_root_name(alias_roots, name.as_str())));
    }
}

#[cfg(feature = "native-backend")]
fn mark_cleanup_root_once(
    alias_roots: &BTreeMap<String, String>,
    already_decrefed: &mut BTreeSet<String>,
    name: &str,
) -> bool {
    already_decrefed.insert(alias_root_name(alias_roots, name).to_string())
}

#[cfg(feature = "native-backend")]
fn cleanup_name_excluded(
    name: &str,
    protected_names: Option<&BTreeSet<String>>,
    param_name_set: &BTreeSet<&str>,
    int_primary_vars: &BTreeSet<String>,
    float_primary_vars: &BTreeSet<String>,
) -> bool {
    protected_names.is_some_and(|protected| protected.contains(name))
        || param_name_set.contains(name)
        || int_primary_vars.contains(name)
        || float_primary_vars.contains(name)
}

#[cfg(feature = "native-backend")]
fn protect_cleanup_names(
    carry: &mut Vec<String>,
    cleanup: Vec<String>,
    protected: &BTreeSet<&str>,
    alias_roots: &BTreeMap<String, String>,
    already_decrefed: &mut BTreeSet<String>,
) -> Vec<String> {
    if protected.is_empty() {
        return cleanup;
    }
    let mut preserved = Vec::new();
    let mut actual = Vec::new();
    for name in cleanup {
        if protected.contains(name.as_str()) {
            already_decrefed.remove(alias_root_name(alias_roots, &name));
            preserved.push(name);
        } else {
            actual.push(name);
        }
    }
    crate::extend_unique_tracked(carry, preserved);
    actual
}

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
        let representation_plan = ScalarRepresentationPlan::for_function_ir(&func_ir);
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
            int_like_vars,
            bool_like_vars,
            float_like_vars,
            str_like_vars,
            none_like_vars,
            has_arena_eligible,
            arena_eligible_outs: _arena_eligible_outs,
            scalar_slot_exclusion_unsafe,
            field_store_modes,
            drop_inserted,
        } = preanalyze_function_ir(&func_ir, return_alias_summaries, &representation_plan);
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
        let native_rc_tracking_enabled = !drop_inserted;
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
        let primary_names = representation_plan.primary_name_sets();
        let int_primary_vars = primary_names.int;
        let bool_primary_vars = primary_names.bool_;
        let float_primary_vars = primary_names.float;
        for name in var_names.iter() {
            let var_type = if float_primary_vars.contains(name) {
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
        // Variable IS the raw i64 carrier for int_primary_vars members.
        // Cranelift's FunctionBuilder inserts phi nodes automatically at
        // block boundaries when a Variable has multiple defs, so the legacy
        // two-tier shadow plumbing is redundant. Reading via
        // `int_raw_value(builder, vars, int_primary_vars, name)` returns the
        // raw i64 directly when name is a static member.

        // Phase 1d: int_primary_vars (declared above ~line 2665 via the
        // operand-recursive fixpoint) is the immutable source of truth for
        // "vars[name] holds raw i64". Float primary lowering follows the
        // same static-set rule for the F64-primary subset.
        // `float_primary_vars` is the immutable source of truth for F64-primary Variables.
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
        // Proven-int join slots that have raw_int_shadow Variables are excluded:
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
                let is_scalar = int_primary_vars.contains(name)
                    || int_like_vars.contains(name)
                    || float_like_vars.contains(name)
                    || bool_like_vars.contains(name);
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
                    int_primary_vars.contains(name.as_str())
                        || bool_primary_vars.contains(name.as_str())
                })
                .cloned()
                .collect()
        } else {
            BTreeSet::new()
        };

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

        let nbc = NanBoxConsts::new(&mut builder);

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
            if bool_primary_vars.contains(name) {
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
                int_primary_vars,
                float_primary_vars,
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
                &int_primary_vars,
                &float_primary_vars,
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
                fc::const_literals::collect_loop_entry_const_defs(&func_ir, &int_primary_vars)
            } else {
                BTreeMap::new()
            };
            let none_val = builder.ins().iconst(types::I64, box_none());
            let float_zero = builder.ins().f64const(0.0);
            for (name, var) in &vars {
                if param_name_set.contains(name.as_str()) {
                    continue;
                }
                if float_primary_vars.contains(name) {
                    // Float-primary: Variable is F64, initialize with f64 zero.
                    builder.def_var(*var, float_zero);
                } else if int_primary_vars.contains(name) {
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
            &int_primary_vars,
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
        let metadata_loop_ops = metadata_only_structured_loop_ops(ops);

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

            // ── Per-iteration dec_ref for loop-body reassigned variables ──
            // When a variable is assigned inside a loop body, the previous
            // iteration's value must be dec_ref'd before the new value is
            // stored.  This mirrors CPython's STORE_FAST semantics where the
            // old slot occupant is dec_ref'd on reassignment.
            //
            // We capture the old SSA Value via use_var *before* the op handler
            // overwrites it with def_var_named.  After the op handler, we emit
            // dec_ref_obj for the old value.  On the first iteration the old
            // value is the None-sentinel (0) we initialized before the loop
            // header, which molt_dec_ref_obj safely ignores (non-pointer).
            let loop_reassign_old_val: Option<Value> = if loop_depth > 0
                // RC drop-insertion substrate (design 20, R1 guard): when the TIR
                // drop pass processed this function it already inserted the
                // loop-carried DecRef; the ad-hoc path below would double-drop.
                && !drop_inserted
                && !is_block_filled
                && let Some(ref name) = out_name
                && name != "none"
                && !rc_skip_dec.contains(name.as_str())
                // Only for ops that can produce heap-allocated refcounted
                // objects — skip constants and loop infrastructure.
                && !matches!(
                    op.kind.as_str(),
                    "const"
                        | "const_str"
                        | "const_bytes"
                        | "const_bigint"
                        | "const_float"
                        | "const_none"
                        | "const_bool"
                        | "loop_index_start"
                        | "loop_index_next"
                        | "loop_break_if_true"
                        | "loop_break_if_false"
                        | "loop_break_if_exception"
                        | "loop_break"
                        | "loop_continue"
                        | "loop_start"
                        | "loop_end"
                        | "phi"
                        | "load_var"
                        | "copy_var"
                        | "store_var"
                        | "delete_var"
                        | "label"
                        | "state_label"
                        | "state_switch"
                        | "state_transition"
                        // Container-aliasing ops: these return the same
                        // container pointer, not a new heap allocation.
                        // dec_ref of the container corrupts cell lists.
                        | "store_index"
                        | "index"
                ) {
                // Check the precomputed loop_body_out_vars: this variable must
                // appear in at least one enclosing loop's assignment set.
                let is_loop_body_var = loop_body_out_vars
                    .values()
                    .any(|bv| bv.iter().any(|v| v == name));
                if is_loop_body_var {
                    vars.get(name.as_str()).map(|var| builder.use_var(*var))
                } else {
                    None
                }
            } else {
                None
            };

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
                        &int_primary_vars,
                        &bool_primary_vars,
                        &float_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
                        &int_like_vars,
                        &bool_like_vars,
                        &loop_stack,
                        scalar_fast_paths_enabled,
                        &representation_plan,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
                        &representation_plan,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
                        &nbc,
                        &bool_like_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
                        scalar_fast_paths_enabled,
                        &representation_plan,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
                        &int_like_vars,
                        &bool_like_vars,
                        &float_like_vars,
                        &str_like_vars,
                        &none_like_vars,
                        &mut list_index_fast_paths,
                        scalar_fast_paths_enabled,
                        &representation_plan,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
                        &int_like_vars,
                        &float_like_vars,
                        &bool_like_vars,
                        &loop_stack,
                        scalar_fast_paths_enabled,
                        &representation_plan,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
                        &int_like_vars,
                        &bool_like_vars,
                        local_inc_ref_obj,
                        scalar_fast_paths_enabled,
                        &representation_plan,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        native_rc_tracking_enabled,
                        returns_value,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        native_rc_tracking_enabled,
                        !loop_stack.is_empty(),
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        drop_inserted,
                        native_rc_tracking_enabled,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
                        &bool_like_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        native_rc_tracking_enabled,
                        &mut self.module,
                        &mut self.import_ids,
                        &mut builder,
                        &mut import_refs,
                        &mut sealed_blocks,
                        &vars,
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &bool_primary_vars,
                        &float_primary_vars,
                        &int_like_vars,
                        &bool_like_vars,
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
                        native_rc_tracking_enabled,
                        scalar_fast_paths_enabled,
                        &representation_plan,
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
                        &int_primary_vars,
                        &bool_primary_vars,
                        &float_primary_vars,
                        &int_like_vars,
                        &bool_like_vars,
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
                        native_rc_tracking_enabled,
                        scalar_fast_paths_enabled,
                        debug_loop_cfg.as_deref(),
                        debug_block_origins.as_deref(),
                        &representation_plan,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
                        &int_like_vars,
                        &float_like_vars,
                        &bool_like_vars,
                        &str_like_vars,
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
                        native_rc_tracking_enabled,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
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
                        &int_primary_vars,
                        &float_primary_vars,
                        &bool_primary_vars,
                        &int_like_vars,
                        &bool_like_vars,
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
                        drop_inserted,
                        native_rc_tracking_enabled,
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

            // ── Emit dec_ref for the old value of loop-body reassigned vars ──
            // The old value was captured via use_var before the op handler ran.
            // Now that def_var_named has stored the new value, dec_ref the old
            // one.  On the first iteration this is the None-sentinel (0) which
            // molt_dec_ref_obj treats as a no-op.
            if let Some(old_val) = loop_reassign_old_val
                && !is_block_filled
            {
                builder.ins().call(local_dec_ref_obj, &[old_val]);
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
                    native_rc_tracking_enabled,
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
                    native_rc_tracking_enabled,
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
                && !drop_inserted
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
                        &int_primary_vars,
                        &float_primary_vars,
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
            if !native_rc_tracking_enabled {
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
                if cleanup_name_excluded(
                    name,
                    None,
                    &param_name_set,
                    &int_primary_vars,
                    &float_primary_vars,
                ) {
                    continue;
                }
                if let Some(val) = entry_vars.get(name)
                    && mark_cleanup_root_once(&alias_roots, &mut already_decrefed, name)
                {
                    builder.ins().call(local_dec_ref_obj, &[*val]);
                }
            }
            for name in &tracked_obj_vars {
                if cleanup_name_excluded(
                    name,
                    None,
                    &param_name_set,
                    &int_primary_vars,
                    &float_primary_vars,
                ) {
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
        if !drop_inserted {
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
        // Typed IR Phase 1a invariant: every variable that the dynamic
        // codegen path classified as raw-primary-int (int_primary_vars) must
        // also have been included in the static int_primary_vars set
        // computed before codegen.  A divergence means either the static
        // analysis is too narrow (under-approximates the runtime decisions
        // — Phases 1b/1c will widen it) or the dynamic path inserted a
        // name that the static analysis cannot prove safe (a correctness
        // hazard, since a future phase will use the static set as the
        // ground truth).  Gated by env var so production builds skip it.
        if std::env::var("MOLT_TYPED_IR_VERIFY").is_ok() {
            for name in int_primary_vars.iter() {
                debug_assert!(
                    int_primary_vars.contains(name),
                    "MOLT_TYPED_IR_VERIFY: int_primary_vars {{{name}}} not in int_primary_vars for fn {}",
                    func_ir.name,
                );
            }
        }
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

/// Release dead block-tracked heap temporaries of the current block at a
/// generator/async `_poll` suspend boundary (`state_yield` / `state_transition`
/// / `chan_*_yield`), immediately before the jump to the master return block.
///
/// A `_poll` returns to its caller on every yield/await and is re-entered on the
/// next resume, so each suspend is the per-iteration scope exit for any heap
/// temporary that is dead before it.  Without this drain those temporaries —
/// chiefly the `(value, done)` pair tuple emitted right before each
/// `state_yield` — are re-allocated and orphaned on every resume, producing an
/// unbounded leak that delegation (`yield from` / manual for-yield) multiplies
/// by the chain depth.
///
/// Only names whose `last_use <= op_idx` are released (the
/// `drain_cleanup_tracked_dedup` gate); loop-carried values keep their
/// func_end-extended `last_use` and therefore survive the suspend.  This is the
/// suspend-boundary twin of the function-return drain in the `ret` handler,
/// restricted to the per-iteration temporaries identified by
/// `stateful_per_iter_temps`.
///
/// Free function (not a method): a live `FunctionBuilder` holds `&mut
/// self.ctx.func`, so a `&mut self` method taking the builder would double-borrow
/// `self` — the same reason the surrounding codegen routes through free helpers.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
fn drain_dead_block_temps_for_suspend(
    native_rc_tracking_enabled: bool,
    builder: &mut FunctionBuilder,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    already_decrefed: &mut BTreeSet<String>,
    entry_vars: &BTreeMap<String, Value>,
    vars: &BTreeMap<String, Variable>,
    local_dec_ref_obj: FuncRef,
    op_idx: usize,
) {
    let Some(block) = builder.current_block() else {
        return;
    };
    for tracked in [block_tracked_obj, block_tracked_ptr] {
        let Some(names) = tracked.get_mut(&block) else {
            continue;
        };
        let cleanup = drain_cleanup_tracked_dedup_with_authority(
            native_rc_tracking_enabled,
            names,
            last_use,
            alias_roots,
            op_idx,
            None,
            Some(already_decrefed),
        );
        for name in cleanup {
            // Prefer the definition-time Value (entry_vars); fall back to the
            // current SSA value of the slot — identical to the `ret` cleanup
            // path (`resolve_cleanup_value`).  For a loop-body temporary the
            // current value is the freshly-defined object, which is exactly
            // what must be released at the suspend.  obj- and ptr-tracked
            // names both release through molt_dec_ref_obj (NaN-box aware).
            let Some(val) = resolve_cleanup_value(builder, vars, entry_vars, &name) else {
                continue;
            };
            builder.ins().call(local_dec_ref_obj, &[val]);
        }
    }
}

#[cfg(all(test, feature = "native-backend"))]
mod tests {
    use super::fc::list_index_fast_path::{
        generic_list_int_lane_eligible, index_fallback_import_name,
        metadata_only_structured_loop_ops, scan_loop_int_sum_reduction,
        store_index_fallback_import_name,
    };
    use super::{
        FieldStoreMode, FunctionPreanalysis, ScalarRepresentationPlan, alias_root_name,
        box_raw_bool_value, box_raw_i64_value_overflow_safe, cleanup_roots_for_names,
        collect_slot_backed_join_names, def_var_from_boxed_transport, def_var_from_numeric_result,
        import_func_ref, is_cold_module_chunk_function, jump_block,
        live_exception_rebind_vars_for_op, mark_cleanup_root_once, materialize_label_block,
        preanalyze_function_ir, protect_cleanup_names, switch_to_block_materialized,
        switch_to_block_with_rebind,
    };
    use crate::{FunctionIR, OpIR, SimpleBackend, SimpleIR};
    use cranelift_codegen::isa::CallConv;
    use cranelift_codegen::{
        ir::{AbiParam, Function, InstBuilder, Signature, UserFuncName, types},
        settings,
        verifier::verify_function,
    };
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
    use std::collections::{BTreeMap, BTreeSet};

    fn preanalyze_for_test(
        func_ir: &FunctionIR,
        return_alias_summaries: &BTreeMap<String, crate::passes::ReturnAliasSummary>,
    ) -> FunctionPreanalysis {
        let representation_plan = ScalarRepresentationPlan::for_function_ir(func_ir);
        preanalyze_function_ir(func_ir, return_alias_summaries, &representation_plan)
    }

    fn representation_plan_for_ops(ops: &[OpIR]) -> ScalarRepresentationPlan {
        ScalarRepresentationPlan::for_function_ir(&FunctionIR {
            name: "storage_test".to_string(),
            params: vec![],
            ops: ops.to_vec(),
            param_types: None,
            source_file: None,
            is_extern: false,
        })
    }

    fn representation_plan_for_typed_ops(
        params: &[&str],
        param_types: Option<Vec<&str>>,
        ops: &[OpIR],
    ) -> ScalarRepresentationPlan {
        ScalarRepresentationPlan::for_function_ir(&FunctionIR {
            name: "container_dispatch_test".to_string(),
            params: params.iter().map(|param| param.to_string()).collect(),
            ops: ops.to_vec(),
            param_types: param_types
                .map(|types| types.into_iter().map(|ty| ty.to_string()).collect()),
            source_file: None,
            is_extern: false,
        })
    }

    fn list_int_new(out: &str) -> OpIR {
        OpIR {
            kind: "list_int_new".to_string(),
            out: Some(out.to_string()),
            ..OpIR::default()
        }
    }

    fn op_kind(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..OpIR::default()
        }
    }

    #[test]
    #[should_panic(expected = "import signature mismatch for molt_test_import")]
    fn import_func_ref_validates_signature_before_local_reuse() {
        let mut backend = SimpleBackend::new();
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut backend.ctx.func, &mut builder_ctx);
        let entry_block = builder.create_block();
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);

        let mut import_refs = BTreeMap::new();
        import_func_ref(
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut import_refs,
            "molt_test_import",
            &[types::I64],
            &[types::I64],
        );
        import_func_ref(
            &mut backend.module,
            &mut backend.import_ids,
            &mut builder,
            &mut import_refs,
            "molt_test_import",
            &[types::I64, types::I64],
            &[types::I64],
        );
    }

    #[test]
    fn metadata_only_structured_loop_ops_skips_unmatched_loop_controls() {
        let ops = vec![
            op_kind("state_switch"),
            op_kind("loop_start"),
            OpIR {
                kind: "loop_break_if_true".to_string(),
                args: Some(vec!["done".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(365),
                ..OpIR::default()
            },
            OpIR {
                kind: "br_if".to_string(),
                args: Some(vec!["done".to_string()]),
                value: Some(343),
                ..OpIR::default()
            },
        ];

        assert_eq!(
            metadata_only_structured_loop_ops(&ops),
            BTreeSet::from([1usize, 2usize]),
            "TIR-linearized label loops must not also lower stale structured loop markers",
        );
    }

    #[test]
    fn metadata_only_structured_loop_ops_preserves_matched_nested_loops() {
        let ops = vec![
            op_kind("loop_start"),
            op_kind("loop_break_if_false"),
            op_kind("loop_start"),
            op_kind("loop_continue"),
            op_kind("loop_end"),
            op_kind("loop_break"),
            op_kind("loop_end"),
        ];

        assert!(
            metadata_only_structured_loop_ops(&ops).is_empty(),
            "well-formed structured loop ranges remain executable native CFG",
        );
    }

    #[test]
    fn native_container_dispatch_uses_tir_container_facts() {
        let dict_index = OpIR {
            kind: "index".to_string(),
            args: Some(vec!["mapping".to_string(), "key".to_string()]),
            out: Some("item".to_string()),
            ..OpIR::default()
        };
        let dict_plan = representation_plan_for_typed_ops(
            &["mapping", "key"],
            Some(vec!["dict[str, int]", "str"]),
            std::slice::from_ref(&dict_index),
        );
        assert_eq!(
            index_fallback_import_name(&dict_plan, &dict_index, false),
            "molt_dict_getitem"
        );

        let tuple_index = OpIR {
            kind: "index".to_string(),
            args: Some(vec!["items".to_string(), "idx".to_string()]),
            out: Some("item".to_string()),
            ..OpIR::default()
        };
        let tuple_plan = representation_plan_for_typed_ops(
            &["items", "idx"],
            Some(vec!["tuple[int, str]", "int"]),
            std::slice::from_ref(&tuple_index),
        );
        assert_eq!(
            index_fallback_import_name(&tuple_plan, &tuple_index, false),
            "molt_tuple_getitem"
        );

        let dict_store = OpIR {
            kind: "store_index".to_string(),
            args: Some(vec![
                "mapping".to_string(),
                "key".to_string(),
                "value".to_string(),
            ]),
            ..OpIR::default()
        };
        let dict_store_plan = representation_plan_for_typed_ops(
            &["mapping", "key", "value"],
            Some(vec!["dict[str, int]", "str", "int"]),
            std::slice::from_ref(&dict_store),
        );
        assert_eq!(
            store_index_fallback_import_name(&dict_store_plan, &dict_store, false),
            "molt_dict_setitem"
        );
    }

    #[test]
    fn native_container_dispatch_ignores_transport_only_container_type() {
        let mut transport_index = OpIR {
            kind: "index".to_string(),
            args: Some(vec!["items".to_string(), "idx".to_string()]),
            out: Some("item".to_string()),
            ..OpIR::default()
        };
        transport_index.container_type = Some("tuple".to_string());
        let plan = representation_plan_for_typed_ops(
            &["items", "idx"],
            None,
            std::slice::from_ref(&transport_index),
        );

        assert_eq!(
            index_fallback_import_name(&plan, &transport_index, false),
            "molt_index"
        );
        assert!(
            !generic_list_int_lane_eligible(&plan, &transport_index, true),
            "transport-only container_type must not enable native generic-list inlining"
        );

        let mut transport_store = OpIR {
            kind: "store_index".to_string(),
            args: Some(vec![
                "mapping".to_string(),
                "key".to_string(),
                "value".to_string(),
            ]),
            ..OpIR::default()
        };
        transport_store.container_type = Some("dict".to_string());
        let store_plan = representation_plan_for_typed_ops(
            &["mapping", "key", "value"],
            None,
            std::slice::from_ref(&transport_store),
        );

        assert_eq!(
            store_index_fallback_import_name(&store_plan, &transport_store, false),
            "molt_store_index"
        );
    }

    #[test]
    fn native_generic_list_inlining_uses_tir_container_facts() {
        let list_index = OpIR {
            kind: "index".to_string(),
            args: Some(vec!["items".to_string(), "idx".to_string()]),
            out: Some("item".to_string()),
            ..OpIR::default()
        };
        let plan = representation_plan_for_typed_ops(
            &["items", "idx"],
            Some(vec!["list[int]", "int"]),
            std::slice::from_ref(&list_index),
        );

        assert!(generic_list_int_lane_eligible(&plan, &list_index, true));
        assert!(!generic_list_int_lane_eligible(&plan, &list_index, false));
    }

    #[test]
    fn raw_bool_boxing_accepts_i64_carrier() {
        let mut sig = Signature::new(CallConv::SystemV);
        sig.returns.push(AbiParam::new(types::I64));
        let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);
        let mut context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut func, &mut context);
            let entry = builder.create_block();
            builder.switch_to_block(entry);
            builder.seal_block(entry);
            let raw = builder.ins().iconst(types::I64, 1);
            let nbc = crate::NanBoxConsts::new(&mut builder);
            let boxed = box_raw_bool_value(&mut builder, raw, &nbc);
            builder.ins().return_(&[boxed]);
            builder.finalize();
        }

        let flags = settings::Flags::new(settings::builder());
        verify_function(&func, &flags).expect("raw bool boxing must verify with an i64 carrier");
    }

    #[test]
    fn native_int_boxing_constants_materialized_at_site() {
        let mut backend = SimpleBackend::new();
        let mut sig = Signature::new(CallConv::SystemV);
        sig.returns.push(AbiParam::new(types::I64));
        let mut func = Function::with_name_signature(UserFuncName::user(0, 1), sig);
        let mut context = FunctionBuilderContext::new();
        let int_mask_needle;
        let int_tag_needle;
        {
            let mut builder = FunctionBuilder::new(&mut func, &mut context);
            let entry = builder.create_block();
            builder.switch_to_block(entry);
            builder.seal_block(entry);

            let nbc = crate::NanBoxConsts::new(&mut builder);
            int_mask_needle = format!("iconst.i64 {:#x}", nbc.int_mask);
            int_tag_needle = format!("iconst.i64 {:#x}", nbc.qnan_tag_int);

            let raw_zero = builder.ins().iconst(types::I64, 0);
            let mut import_refs = BTreeMap::new();
            let mut sealed_blocks = BTreeSet::from([entry]);
            let boxed = box_raw_i64_value_overflow_safe(
                &mut backend.module,
                &mut backend.import_ids,
                &mut builder,
                &mut import_refs,
                &mut sealed_blocks,
                raw_zero,
            );
            builder.ins().return_(&[boxed]);
            builder.finalize();
        }

        let flags = settings::Flags::new(settings::builder());
        verify_function(&func, &flags).expect("split raw-i64 escape boxing CFG must verify");
        let clif = func.display().to_string();
        let normalized_clif = clif.replace('_', "");
        assert!(
            normalized_clif.contains(&int_mask_needle),
            "raw-i64 escape boxing must materialize INT_MASK at the boxing site:\n{clif}"
        );
        assert!(
            normalized_clif.contains(&int_tag_needle),
            "raw-i64 escape boxing must materialize QNAN|TAG_INT at the boxing site:\n{clif}"
        );
    }

    #[test]
    fn boxed_transport_defines_scalar_primary_homes() {
        let mut backend = SimpleBackend::new();
        let mut sig = Signature::new(CallConv::SystemV);
        sig.returns.push(AbiParam::new(types::I64));
        let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);
        let mut context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut func, &mut context);
            let entry = builder.create_block();
            builder.switch_to_block(entry);
            builder.seal_block(entry);

            let float_var = builder.declare_var(types::F64);
            let bool_var = builder.declare_var(types::I64);
            let int_var = builder.declare_var(types::I64);
            let mut vars = BTreeMap::new();
            vars.insert("float_home".to_string(), float_var);
            vars.insert("bool_home".to_string(), bool_var);
            vars.insert("int_home".to_string(), int_var);

            let int_primary_vars = BTreeSet::from(["int_home".to_string()]);
            let bool_primary_vars = BTreeSet::from(["bool_home".to_string()]);
            let float_primary_vars = BTreeSet::from(["float_home".to_string()]);
            let mut import_refs = BTreeMap::new();
            let nbc = crate::NanBoxConsts::new(&mut builder);

            let boxed_float = builder.ins().iconst(types::I64, 1.25f64.to_bits() as i64);
            def_var_from_boxed_transport(
                &mut backend.module,
                &mut backend.import_ids,
                &mut builder,
                &mut import_refs,
                &vars,
                &int_primary_vars,
                &bool_primary_vars,
                &float_primary_vars,
                &nbc,
                "float_home",
                boxed_float,
            );

            let raw_bool = builder.ins().iconst(types::I64, 1);
            let boxed_bool = box_raw_bool_value(&mut builder, raw_bool, &nbc);
            def_var_from_boxed_transport(
                &mut backend.module,
                &mut backend.import_ids,
                &mut builder,
                &mut import_refs,
                &vars,
                &int_primary_vars,
                &bool_primary_vars,
                &float_primary_vars,
                &nbc,
                "bool_home",
                boxed_bool,
            );

            let boxed_int = builder.ins().iconst(types::I64, nbc.qnan_tag_int | 7);
            def_var_from_boxed_transport(
                &mut backend.module,
                &mut backend.import_ids,
                &mut builder,
                &mut import_refs,
                &vars,
                &int_primary_vars,
                &bool_primary_vars,
                &float_primary_vars,
                &nbc,
                "int_home",
                boxed_int,
            );

            let raw_int = builder.use_var(int_var);
            builder.ins().return_(&[raw_int]);
            builder.finalize();
        }

        let flags = settings::Flags::new(settings::builder());
        verify_function(&func, &flags)
            .expect("boxed transport must define scalar-primary homes with matching CLIF types");
    }

    #[test]
    fn numeric_result_binding_converts_boxed_call_result_for_float_primary_home() {
        let mut backend = SimpleBackend::new();
        let mut sig = Signature::new(CallConv::SystemV);
        sig.returns.push(AbiParam::new(types::F64));
        let mut func = Function::with_name_signature(UserFuncName::user(0, 0), sig);
        let mut context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut func, &mut context);
            let entry = builder.create_block();
            builder.switch_to_block(entry);
            builder.seal_block(entry);

            let float_var = builder.declare_var(types::F64);
            let mut vars = BTreeMap::new();
            vars.insert("float_home".to_string(), float_var);

            let int_primary_vars = BTreeSet::new();
            let bool_primary_vars = BTreeSet::new();
            let float_primary_vars = BTreeSet::from(["float_home".to_string()]);
            let mut import_refs = BTreeMap::new();
            let nbc = crate::NanBoxConsts::new(&mut builder);

            let boxed_float = builder.ins().iconst(types::I64, 1.25f64.to_bits() as i64);
            def_var_from_numeric_result(
                &mut backend.module,
                &mut backend.import_ids,
                &mut builder,
                &mut import_refs,
                &vars,
                &int_primary_vars,
                &bool_primary_vars,
                &float_primary_vars,
                &nbc,
                "float_home",
                boxed_float,
            );

            let raw_f64 = builder.use_var(float_var);
            builder.ins().return_(&[raw_f64]);
            builder.finalize();
        }

        let flags = settings::Flags::new(settings::builder());
        verify_function(&func, &flags)
            .expect("boxed call result must bind to float-primary homes as raw f64");
    }

    #[test]
    fn semantic_type_hint_does_not_create_native_scalar_lane_for_generic_ops() {
        let hinted_generic_op = OpIR {
            kind: "call_indirect".to_string(),
            args: Some(vec!["callable".to_string(), "args".to_string()]),
            out: Some("result".to_string()),
            type_hint: Some("int".to_string()),
            ..OpIR::default()
        };

        let func = FunctionIR {
            name: "hinted_generic".to_string(),
            params: vec!["callable".to_string(), "args".to_string()],
            ops: vec![hinted_generic_op],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert!(
            !analysis.int_like_vars.contains("result"),
            "preanalysis must keep generic runtime results boxed even when type_hint=int",
        );
    }

    #[test]
    fn preanalysis_keeps_mixed_join_store_targets_boxed() {
        let func = FunctionIR {
            name: "mixed_join".to_string(),
            params: vec!["callable".to_string(), "args".to_string()],
            ops: vec![
                OpIR {
                    kind: "call_indirect".to_string(),
                    args: Some(vec!["callable".to_string(), "args".to_string()]),
                    out: Some("dynamic".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("_bb_arg0".to_string()),
                    args: Some(vec!["dynamic".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_bool".to_string(),
                    out: Some("fallback".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("_bb_arg0".to_string()),
                    args: Some(vec!["fallback".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".to_string(),
                    var: Some("_bb_arg0".to_string()),
                    out: Some("joined".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        for name in ["_bb_arg0", "joined"] {
            assert!(
                !analysis.bool_like_vars.contains(name)
                    && !analysis.int_like_vars.contains(name)
                    && !analysis.float_like_vars.contains(name)
                    && !analysis.str_like_vars.contains(name)
                    && !analysis.none_like_vars.contains(name),
                "mixed dynamic/scalar join target {name} must stay boxed",
            );
        }
    }

    #[test]
    fn preanalysis_keeps_unbounded_integer_family_out_of_float_lane() {
        let func = FunctionIR {
            name: "integer_family_chain".to_string(),
            params: vec!["x".to_string(), "seed".to_string()],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    value: Some(374761393),
                    out: Some("_v0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "mul".to_string(),
                    args: Some(vec!["x".to_string(), "_v0".to_string()]),
                    out: Some("_v1".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "bit_xor".to_string(),
                    args: Some(vec!["seed".to_string(), "_v1".to_string()]),
                    out: Some("_v2".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(13),
                    out: Some("_v3".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "rshift".to_string(),
                    args: Some(vec!["_v2".to_string(), "_v3".to_string()]),
                    out: Some("_v4".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "bit_xor".to_string(),
                    args: Some(vec!["_v2".to_string(), "_v4".to_string()]),
                    out: Some("_v5".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(3266489917),
                    out: Some("_v6".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "mul".to_string(),
                    args: Some(vec!["_v5".to_string(), "_v6".to_string()]),
                    out: Some("_v7".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
        };

        let plan = ScalarRepresentationPlan::for_function_ir(&func);
        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert!(plan.integer_family_names().contains("_v7"));
        assert!(!analysis.int_like_vars.contains("_v7"));
        assert!(!analysis.float_like_vars.contains("_v7"));
    }

    #[test]
    fn native_backend_compiles_float_primary_tuple_escape_before_exception_cleanup() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "float_primary_tuple_cleanup".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("src_a".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "float_from_obj".to_string(),
                        out: Some("flt_a".to_string()),
                        args: Some(vec!["src_a".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("src_b".to_string()),
                        value: Some(2),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "float_from_obj".to_string(),
                        out: Some("flt_b".to_string()),
                        args: Some(vec!["src_b".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("src_c".to_string()),
                        value: Some(3),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "float_from_obj".to_string(),
                        out: Some("flt_c".to_string()),
                        args: Some(vec!["src_c".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "tuple_new".to_string(),
                        out: Some("loads".to_string()),
                        args: Some(vec![
                            "flt_a".to_string(),
                            "flt_b".to_string(),
                            "flt_c".to_string(),
                        ]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "check_exception".to_string(),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("loads".to_string()),
                        args: Some(vec!["loads".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "label".to_string(),
                        value: Some(7),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "const_none".to_string(),
                        out: Some("none_ret".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("none_ret".to_string()),
                        args: Some(vec!["none_ret".to_string()]),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let output = SimpleBackend::new().compile(ir);

        assert!(!output.bytes.is_empty());
    }

    fn compile_retained_alias_after_source_dec_ref(alias_kind: &str) {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: format!("{alias_kind}_after_dec_ref"),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "const_str".to_string(),
                        out: Some("src".to_string()),
                        s_value: Some("owned".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: alias_kind.to_string(),
                        args: Some(vec!["src".to_string()]),
                        out: Some("alias".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "dec_ref".to_string(),
                        args: Some(vec!["src".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("alias".to_string()),
                        args: Some(vec!["alias".to_string()]),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            }],
            profile: None,
        };

        let output = SimpleBackend::new().compile(ir);

        assert!(!output.bytes.is_empty());
    }

    #[test]
    fn native_backend_compiles_identity_alias_after_source_dec_ref() {
        compile_retained_alias_after_source_dec_ref("identity_alias");
    }

    #[test]
    fn native_backend_compiles_binding_alias_after_source_dec_ref() {
        compile_retained_alias_after_source_dec_ref("binding_alias");
    }

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
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert!(analysis.has_ret);
        assert!(analysis.stateful);
        assert_eq!(analysis.if_to_end_if.get(&1), Some(&4));
        assert_eq!(analysis.if_to_else.get(&1), Some(&3));
        assert_eq!(analysis.else_to_end_if.get(&3), Some(&4));
        assert_eq!(analysis.state_ids, vec![7, 42]);
        assert_eq!(analysis.label_ids, vec![42]);
        assert!(analysis.state_label_ids.contains(&42));
        assert!(!analysis.state_label_ids.contains(&7));
        assert!(analysis.shared_resume_label_ids.contains(&42));
        assert!(!analysis.shared_resume_label_ids.contains(&7));
        assert!(analysis.resume_states.contains(&7));
        assert!(analysis.resume_states.contains(&42));
        assert_eq!(analysis.function_exception_label_id, Some(42));
        assert!(analysis.var_names.contains(&"msg_ptr".to_string()));
        assert!(analysis.var_names.contains(&"msg_len".to_string()));
        // After alias analysis, "msg" and "out" share the same alias root
        // (copy propagation makes "out" an alias of "msg"), so both last_use
        // values are extended to the maximum of the group (op 9, the ret op).
        assert_eq!(analysis.last_use.get("msg"), Some(&9));
        assert_eq!(analysis.last_use.get("out"), Some(&9));
    }

    #[test]
    fn preanalysis_distinguishes_ret_from_ret_void() {
        let value_ret = FunctionIR {
            name: "value_ret".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret".to_string(),
                var: Some("out".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };
        let void_ret = FunctionIR {
            name: "void_ret".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        assert!(
            preanalyze_for_test(&value_ret, &BTreeMap::new()).has_ret,
            "`ret` should mark the function as value-returning"
        );
        assert!(
            !preanalyze_for_test(&void_ret, &BTreeMap::new()).has_ret,
            "`ret_void` must not mark the function as value-returning"
        );
    }

    #[test]
    fn preanalysis_marks_every_persisted_coroutine_state_resumable() {
        let func = FunctionIR {
            name: "stateful_ready_continuations".to_string(),
            params: vec!["self".to_string()],
            ops: vec![
                OpIR {
                    kind: "state_label".to_string(),
                    value: Some(216),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "state_transition".to_string(),
                    args: Some(vec![
                        "future".to_string(),
                        "await_slot".to_string(),
                        "pending_state".to_string(),
                    ]),
                    value: Some(217),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "chan_send_yield".to_string(),
                    args: Some(vec![
                        "chan".to_string(),
                        "value".to_string(),
                        "pending_state".to_string(),
                    ]),
                    value: Some(301),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "chan_recv_yield".to_string(),
                    args: Some(vec!["chan".to_string(), "pending_state".to_string()]),
                    value: Some(302),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert!(
            analysis.resume_states.contains(&216),
            "textual state labels remain dispatchable resume states",
        );
        assert!(
            analysis.resume_states.contains(&217),
            "state_transition ready continuations are stored in object state and must dispatch",
        );
        assert!(
            analysis.resume_states.contains(&301),
            "channel send ready continuations are stored in object state and must dispatch",
        );
        assert!(
            analysis.resume_states.contains(&302),
            "channel receive ready continuations are stored in object state and must dispatch",
        );
    }

    #[test]
    fn preanalysis_keeps_regular_labels_distinct_from_resume_state_collisions() {
        let func = FunctionIR {
            name: "resume_label_collision".to_string(),
            params: vec!["self".to_string()],
            ops: vec![
                OpIR {
                    kind: "state_label".to_string(),
                    value: Some(12),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("pending_state".to_string()),
                    value: Some(12),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "state_transition".to_string(),
                    args: Some(vec![
                        "future".to_string(),
                        "await_slot".to_string(),
                        "pending_state".to_string(),
                    ]),
                    value: Some(13),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(13),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert_eq!(analysis.label_ids, vec![12, 13]);
        assert!(analysis.resume_states.contains(&12));
        assert!(analysis.resume_states.contains(&13));
        assert!(analysis.state_label_ids.contains(&12));
        assert!(analysis.shared_resume_label_ids.contains(&12));
        assert!(
            !analysis.state_label_ids.contains(&13),
            "a plain label with the same numeric id as a ready continuation must not share its resume block",
        );
        assert!(
            !analysis.shared_resume_label_ids.contains(&13),
            "a plain label collision is not a persisted pending label and must stay separate",
        );
    }

    #[test]
    fn preanalysis_marks_pending_plain_labels_as_shared_resume_entries() {
        let func = FunctionIR {
            name: "pending_plain_label".to_string(),
            params: vec!["self".to_string()],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("pending_state".to_string()),
                    value: Some(12),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "state_transition".to_string(),
                    args: Some(vec![
                        "future".to_string(),
                        "await_slot".to_string(),
                        "pending_state".to_string(),
                    ]),
                    value: Some(13),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(12),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert_eq!(analysis.label_ids, vec![12]);
        assert!(analysis.resume_states.contains(&12));
        assert!(analysis.resume_states.contains(&13));
        assert!(!analysis.state_label_ids.contains(&12));
        assert!(analysis.shared_resume_label_ids.contains(&12));
        assert!(
            !analysis.shared_resume_label_ids.contains(&13),
            "ready-continuation states use dedicated resume blocks unless a textual label is actually persisted",
        );
    }

    #[test]
    fn preanalysis_treats_immediate_fresh_object_field_stores_as_direct() {
        let func = FunctionIR {
            name: "stack_field_store".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("cls".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "object_new_bound_stack".to_string(),
                    out: Some("obj".to_string()),
                    args: Some(vec!["cls".to_string()]),
                    value: Some(24),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("zero".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_init".to_string(),
                    args: Some(vec!["obj".to_string(), "zero".to_string()]),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "copy".to_string(),
                    out: Some("alias".to_string()),
                    args: Some(vec!["obj".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("one".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store".to_string(),
                    args: Some(vec!["alias".to_string(), "one".to_string()]),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert!(
            !analysis.has_store,
            "immediate stores into fresh stack object slots should lower as direct field writes"
        );
        assert_eq!(
            analysis.field_store_modes.get(&3),
            Some(&FieldStoreMode::FreshInit),
            "the init write owns fresh-slot initialization semantics"
        );
        assert_eq!(
            analysis.field_store_modes.get(&6),
            Some(&FieldStoreMode::DirectNonHeap),
            "the later same-slot immediate write should be direct"
        );
    }

    #[test]
    fn preanalysis_treats_immediate_heap_fixed_layout_field_stores_as_direct() {
        let func = FunctionIR {
            name: "heap_fixed_layout_field_store".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("cls".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "object_new_bound".to_string(),
                    out: Some("obj".to_string()),
                    args: Some(vec!["cls".to_string()]),
                    value: Some(24),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("zero".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_init".to_string(),
                    args: Some(vec!["obj".to_string(), "zero".to_string()]),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("p".to_string()),
                    args: Some(vec!["obj".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".to_string(),
                    var: Some("p".to_string()),
                    out: Some("alias".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("one".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store".to_string(),
                    args: Some(vec!["alias".to_string(), "one".to_string()]),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert!(
            !analysis.has_store,
            "non-heap stores into fresh fixed-layout heap object slots should lower as direct field writes"
        );
        assert_eq!(
            analysis.field_store_modes.get(&3),
            Some(&FieldStoreMode::FreshInit),
            "sized object_new_bound roots should initialize fixed payload slots"
        );
        assert_eq!(
            analysis.field_store_modes.get(&7),
            Some(&FieldStoreMode::DirectNonHeap),
            "sized object_new_bound roots should share the stack-object direct-store contract"
        );
    }

    #[test]
    fn preanalysis_rejects_unsized_heap_object_direct_field_stores() {
        let func = FunctionIR {
            name: "unsized_heap_field_store".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("cls".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "object_new_bound".to_string(),
                    out: Some("obj".to_string()),
                    args: Some(vec!["cls".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("zero".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_init".to_string(),
                    args: Some(vec!["obj".to_string(), "zero".to_string()]),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("p".to_string()),
                    args: Some(vec!["obj".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".to_string(),
                    var: Some("p".to_string()),
                    out: Some("alias".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("one".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store".to_string(),
                    args: Some(vec!["alias".to_string(), "one".to_string()]),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert!(
            analysis.has_store,
            "heap object stores without a fixed payload-size proof must keep runtime field helpers"
        );
        assert!(analysis.field_store_modes.is_empty());
    }

    #[test]
    fn preanalysis_classifies_fresh_heap_field_first_store_as_init() {
        let func = FunctionIR {
            name: "fresh_heap_first_store".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("cls".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "object_new_bound".to_string(),
                    out: Some("obj".to_string()),
                    args: Some(vec!["cls".to_string()]),
                    value: Some(24),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dict_new".to_string(),
                    out: Some("regs".to_string()),
                    args: Some(vec![]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store".to_string(),
                    args: Some(vec!["obj".to_string(), "regs".to_string()]),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert_eq!(
            analysis.field_store_modes.get(&3),
            Some(&FieldStoreMode::FreshInit),
            "first heap-valued write to a fresh fixed-layout slot must not use overwrite semantics"
        );
    }

    #[test]
    fn preanalysis_keeps_heap_field_second_store_as_overwrite() {
        let func = FunctionIR {
            name: "fresh_heap_second_store".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("cls".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "object_new_bound".to_string(),
                    out: Some("obj".to_string()),
                    args: Some(vec!["cls".to_string()]),
                    value: Some(24),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dict_new".to_string(),
                    out: Some("first".to_string()),
                    args: Some(vec![]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store".to_string(),
                    args: Some(vec!["obj".to_string(), "first".to_string()]),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dict_new".to_string(),
                    out: Some("second".to_string()),
                    args: Some(vec![]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store".to_string(),
                    args: Some(vec!["obj".to_string(), "second".to_string()]),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert_eq!(
            analysis.field_store_modes.get(&3),
            Some(&FieldStoreMode::FreshInit)
        );
        assert!(
            !analysis.field_store_modes.contains_key(&5),
            "second heap write to the same slot must stay generic overwrite so the old dict is released"
        );
    }

    #[test]
    fn preanalysis_rejects_fresh_init_after_escape() {
        let func = FunctionIR {
            name: "fresh_store_after_escape".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("cls".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "object_new_bound".to_string(),
                    out: Some("obj".to_string()),
                    args: Some(vec!["cls".to_string()]),
                    value: Some(24),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call".to_string(),
                    args: Some(vec!["obj".to_string()]),
                    out: Some("escaped".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dict_new".to_string(),
                    out: Some("regs".to_string()),
                    args: Some(vec![]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store".to_string(),
                    args: Some(vec!["obj".to_string(), "regs".to_string()]),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert!(
            !analysis.field_store_modes.contains_key(&4),
            "once the object escapes, first-write init semantics are no longer locally provable"
        );
    }

    #[test]
    fn preanalysis_treats_store_var_join_slot_as_alias_definition() {
        let func = FunctionIR {
            name: "join_alias".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("src".to_string()),
                    s_value: Some("hi".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("_bb4_arg0".to_string()),
                    args: Some(vec!["src".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".to_string(),
                    var: Some("_bb4_arg0".to_string()),
                    out: Some("joined".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("joined".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert_eq!(
            analysis.alias_roots.get("_bb4_arg0").map(String::as_str),
            Some("src")
        );
        assert_eq!(
            analysis.alias_roots.get("joined").map(String::as_str),
            Some("src")
        );
        assert_eq!(analysis.last_use.get("src"), Some(&3));
        assert_eq!(analysis.last_use.get("_bb4_arg0"), Some(&3));
    }

    #[test]
    fn preanalysis_uses_args_based_copy_var_value_source() {
        let func = FunctionIR {
            name: "args_copy_alias".to_string(),
            params: vec!["value".to_string(), "metadata_slot".to_string()],
            ops: vec![
                OpIR {
                    kind: "copy_var".to_string(),
                    var: Some("metadata_slot".to_string()),
                    args: Some(vec!["value".to_string()]),
                    out: Some("alias".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("alias".to_string()),
                    args: Some(vec!["alias".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert_eq!(
            analysis.alias_roots.get("alias").map(String::as_str),
            Some("value"),
            "args[0] is the copied value authority; var is local-name metadata"
        );
        assert_eq!(analysis.last_use.get("value"), Some(&1));
        assert_eq!(analysis.last_use.get("metadata_slot"), Some(&0));
    }

    #[test]
    fn cold_module_chunk_codegen_classification_only_matches_module_chunks() {
        assert!(is_cold_module_chunk_function(
            "molt_gpu_tensor__molt_module_chunk_2"
        ));
        assert!(is_cold_module_chunk_function(
            "builtins__molt_module_chunk_4"
        ));
        assert!(!is_cold_module_chunk_function(
            "main_molt__Attention___call__"
        ));
        assert!(!is_cold_module_chunk_function(
            "molt_gpu_tensor__Tensor__broadcast_op"
        ));
        assert!(!is_cold_module_chunk_function("molt_main"));
    }

    #[test]
    fn live_exception_rebind_vars_skip_future_definitions() {
        let mut vars = BTreeMap::new();
        vars.insert("early".to_string(), Variable::from_u32(0));
        vars.insert("late".to_string(), Variable::from_u32(1));
        vars.insert("dead".to_string(), Variable::from_u32(2));

        let mut transport_last_use = BTreeMap::new();
        transport_last_use.insert("early".to_string(), 10usize);
        transport_last_use.insert("late".to_string(), 10usize);
        transport_last_use.insert("dead".to_string(), 1usize);

        let mut first_defined_at = BTreeMap::new();
        first_defined_at.insert("early".to_string(), 0usize);
        first_defined_at.insert("late".to_string(), 5usize);
        first_defined_at.insert("dead".to_string(), 0usize);

        let live =
            live_exception_rebind_vars_for_op(&vars, &transport_last_use, &first_defined_at, 3);

        assert!(live.contains_key("early"));
        assert!(!live.contains_key("late"));
        assert!(!live.contains_key("dead"));
    }

    #[test]
    fn preanalysis_marks_unused_outputs_live_through_their_definition_site() {
        let func = FunctionIR {
            name: "unused_delete_temp".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "load_var".to_string(),
                    var: Some("item".to_string()),
                    out: Some("tmp_loaded".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "missing".to_string(),
                    out: Some("tmp_missing".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("item".to_string()),
                    args: Some(vec!["tmp_missing".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert_eq!(analysis.last_use.get("tmp_loaded"), Some(&0));
        assert_eq!(analysis.last_use.get("tmp_missing"), Some(&2));
    }

    #[test]
    fn preanalysis_only_marks_store_slots_as_loop_body_reassignments() {
        let func = FunctionIR {
            name: "loop_store_slot_only".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "loop_start".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("tmp".to_string()),
                    s_value: Some("hi".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("slot".to_string()),
                    args: Some(vec!["tmp".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("v116".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("_v7".to_string()),
                    args: Some(vec!["v116".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_continue".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert_eq!(
            analysis.loop_body_out_vars.get(&0),
            Some(&vec!["slot".to_string()]),
            "loop-body slot tracking should ignore SSA temps and only keep slot-backed reassignments",
        );
        assert_eq!(
            analysis.loop_body_init_vars.get(&0),
            Some(&vec!["slot".to_string()]),
            "slot-backed loop vars without any pre-loop store need an explicit first-iteration sentinel",
        );
    }

    #[test]
    fn preanalysis_does_not_reinitialize_loop_slots_with_preloop_store() {
        let func = FunctionIR {
            name: "loop_store_slot_preinit".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_bool".to_string(),
                    out: Some("v0".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("slot".to_string()),
                    args: Some(vec!["v0".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_start".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_bool".to_string(),
                    out: Some("v1".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("slot".to_string()),
                    args: Some(vec!["v1".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_continue".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());

        assert_eq!(
            analysis.loop_body_out_vars.get(&2),
            Some(&vec!["slot".to_string()]),
            "loop cleanup still needs to track the slot as loop-carried",
        );
        assert!(
            analysis
                .loop_body_init_vars
                .get(&2)
                .is_none_or(|names| !names.iter().any(|name| name == "slot")),
            "pre-loop stores must not be clobbered by synthetic None initialization",
        );
    }

    #[test]
    fn slot_backed_join_names_skip_load_only_phi_join_carriers() {
        let ops = vec![
            OpIR {
                kind: "phi".to_string(),
                out: Some("joined".to_string()),
                args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(18),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".to_string(),
                var: Some("_bb4_arg0".to_string()),
                out: Some("joined".to_string()),
                ..OpIR::default()
            },
        ];

        let names = collect_slot_backed_join_names(&ops, &BTreeSet::new(), false);

        assert!(
            !names.contains("_bb4_arg0"),
            "load-only structured phi join carriers must stay on the SSA path",
        );
    }

    #[test]
    fn slot_backed_join_names_keep_explicit_store_backed_join_carriers() {
        let ops = vec![
            OpIR {
                kind: "store_var".to_string(),
                var: Some("_bb4_arg0".to_string()),
                args: Some(vec!["src".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(18),
                ..OpIR::default()
            },
            OpIR {
                kind: "load_var".to_string(),
                var: Some("_bb4_arg0".to_string()),
                out: Some("joined".to_string()),
                ..OpIR::default()
            },
        ];

        let names = collect_slot_backed_join_names(&ops, &BTreeSet::new(), false);

        assert!(
            names.contains("_bb4_arg0"),
            "explicit store-backed join carriers must remain slot-backed",
        );
    }

    #[test]
    fn exception_slot_backing_ignores_compiler_value_temps() {
        let ops = vec![
            OpIR {
                kind: "store_var".to_string(),
                var: Some("_bb4_arg0".to_string()),
                args: Some(vec!["seed".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("slot".to_string()),
                args: Some(vec!["seed".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("_v7".to_string()),
                args: Some(vec!["seed".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("v116".to_string()),
                args: Some(vec!["seed".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_start".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("_v8".to_string()),
                args: Some(vec!["seed".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("handler_slot".to_string()),
                args: Some(vec!["seed".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_pop".to_string(),
                ..OpIR::default()
            },
        ];
        let exception_labels = BTreeSet::from([7]);

        let names = collect_slot_backed_join_names(&ops, &exception_labels, false);

        assert!(names.contains("_bb4_arg0"));
        assert!(names.contains("slot"));
        assert!(names.contains("handler_slot"));
        for temp in ["_v7", "v116", "_v8"] {
            assert!(
                !names.contains(temp),
                "compiler value temp {temp} must not become exception slot-backed"
            );
        }
    }

    #[test]
    fn cleanup_roots_collapse_join_alias_duplicates() {
        let func = FunctionIR {
            name: "join_alias_cleanup".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    out: Some("src".to_string()),
                    s_value: Some("hi".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("_bb4_arg0".to_string()),
                    args: Some(vec!["src".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".to_string(),
                    var: Some("_bb4_arg0".to_string()),
                    out: Some("joined".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "copy_var".to_string(),
                    var: Some("joined".to_string()),
                    out: Some("arg_alias".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call".to_string(),
                    s_value: Some("callee".to_string()),
                    args: Some(vec!["arg_alias".to_string()]),
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
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());
        let arg_cleanup_roots =
            cleanup_roots_for_names(&analysis.alias_roots, ["arg_alias".to_string()]);

        assert_eq!(arg_cleanup_roots, BTreeSet::from(["src".to_string()]));
        assert!(arg_cleanup_roots.contains(alias_root_name(&analysis.alias_roots, "_bb4_arg0")));
        assert!(arg_cleanup_roots.contains(alias_root_name(&analysis.alias_roots, "joined")));
    }

    #[test]
    fn cleanup_root_marking_dedups_aliases() {
        let alias_roots = BTreeMap::from([
            ("alias".to_string(), "root".to_string()),
            ("join".to_string(), "root".to_string()),
        ]);
        let mut already_decrefed = BTreeSet::new();

        assert!(mark_cleanup_root_once(
            &alias_roots,
            &mut already_decrefed,
            "alias",
        ));
        assert!(!mark_cleanup_root_once(
            &alias_roots,
            &mut already_decrefed,
            "join",
        ));
        assert!(!mark_cleanup_root_once(
            &alias_roots,
            &mut already_decrefed,
            "root",
        ));
        assert_eq!(already_decrefed, BTreeSet::from(["root".to_string()]));
    }

    #[test]
    fn protected_cleanup_rearms_preserved_alias_root() {
        let alias_roots = BTreeMap::from([("phi_in".to_string(), "src".to_string())]);
        let protected = BTreeSet::from(["phi_in"]);
        let cleanup = vec!["phi_in".to_string(), "dead".to_string()];
        let mut carry = Vec::new();
        let mut already_decrefed = BTreeSet::from(["src".to_string(), "dead".to_string()]);

        let actual = protect_cleanup_names(
            &mut carry,
            cleanup,
            &protected,
            &alias_roots,
            &mut already_decrefed,
        );

        assert_eq!(carry, vec!["phi_in".to_string()]);
        assert_eq!(actual, vec!["dead".to_string()]);
        assert!(!already_decrefed.contains("src"));
        assert!(already_decrefed.contains("dead"));
    }

    #[test]
    fn switch_to_block_with_rebind_does_not_inflate_merge_params_for_invariant_vars() {
        let mut sig = Signature::new(CallConv::SystemV);
        sig.returns.push(AbiParam::new(types::I64));
        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

        let stable_var = builder.declare_var(types::I64);
        let phi_var = builder.declare_var(types::I64);

        let entry = builder.create_block();
        let then_block = builder.create_block();
        let else_block = builder.create_block();
        let merge_block = builder.create_block();
        builder.append_block_param(merge_block, types::I64);

        switch_to_block_materialized(&mut builder, entry);
        let stable = builder.ins().iconst(types::I64, 7);
        let cond = builder.ins().iconst(types::I8, 1);
        let then_val = builder.ins().iconst(types::I64, 11);
        let else_val = builder.ins().iconst(types::I64, 13);
        builder.def_var(stable_var, stable);
        builder.ins().brif(cond, then_block, &[], else_block, &[]);
        builder.seal_block(entry);

        switch_to_block_materialized(&mut builder, then_block);
        builder.def_var(phi_var, then_val);
        jump_block(&mut builder, merge_block, &[then_val]);
        builder.seal_block(then_block);

        switch_to_block_materialized(&mut builder, else_block);
        builder.def_var(phi_var, else_val);
        jump_block(&mut builder, merge_block, &[else_val]);
        builder.seal_block(else_block);

        let mut is_block_filled = false;
        switch_to_block_with_rebind(&mut builder, merge_block, &mut is_block_filled, false);

        assert_eq!(
            builder.block_params(merge_block).len(),
            1,
            "merge block should only carry the explicit phi payload param",
        );
    }

    #[test]
    fn switch_to_block_with_rebind_does_not_create_exception_fallthrough_phis_for_invariants() {
        let mut sig = Signature::new(CallConv::SystemV);
        sig.returns.push(AbiParam::new(types::I64));
        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

        let stable_var = builder.declare_var(types::I64);

        let entry = builder.create_block();
        let validate_block = builder.create_block();
        let fallthrough = builder.create_block();

        switch_to_block_materialized(&mut builder, entry);
        let stable = builder.ins().iconst(types::I64, 7);
        let cond = builder.ins().iconst(types::I8, 1);
        builder.def_var(stable_var, stable);
        builder
            .ins()
            .brif(cond, validate_block, &[], fallthrough, &[]);
        builder.seal_block(entry);

        switch_to_block_materialized(&mut builder, validate_block);
        builder.ins().jump(fallthrough, &[]);
        builder.seal_block(validate_block);

        let mut is_block_filled = false;
        switch_to_block_with_rebind(&mut builder, fallthrough, &mut is_block_filled, true);

        assert!(
            builder.block_params(fallthrough).is_empty(),
            "exception fallthrough should not synthesize params for invariant vars",
        );
    }

    #[test]
    fn switch_to_block_with_rebind_does_not_create_params_for_plain_label_blocks() {
        let mut sig = Signature::new(CallConv::SystemV);
        sig.returns.push(AbiParam::new(types::I64));
        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

        let stable_var = builder.declare_var(types::I64);

        let entry = builder.create_block();
        let label_block = builder.create_block();

        switch_to_block_materialized(&mut builder, entry);
        let stable = builder.ins().iconst(types::I64, 7);
        builder.def_var(stable_var, stable);
        jump_block(&mut builder, label_block, &[]);
        builder.seal_block(entry);

        let mut is_block_filled = false;
        switch_to_block_with_rebind(&mut builder, label_block, &mut is_block_filled, false);

        assert!(
            builder.block_params(label_block).is_empty(),
            "plain label blocks must not gain implicit params from SSA repair",
        );
    }

    #[test]
    fn materialize_label_block_defines_unreached_forward_label() {
        let sig = Signature::new(CallConv::SystemV);
        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

        let entry = builder.create_block();
        let later = builder.create_block();
        let detached_label = builder.create_block();

        switch_to_block_materialized(&mut builder, entry);
        builder.ins().jump(later, &[]);
        builder.seal_block(entry);

        let mut is_block_filled = true;
        materialize_label_block(&mut builder, detached_label, &mut is_block_filled);

        assert!(
            builder.func.layout.is_block_inserted(detached_label),
            "textual label must materialize its block even before any emitted predecessor reaches it",
        );
        assert_eq!(builder.current_block(), Some(detached_label));
        assert!(
            !is_block_filled,
            "materialized label block must be open for emission"
        );
    }

    #[test]
    fn materialize_label_block_does_not_self_jump_current_resume_block() {
        let sig = Signature::new(CallConv::SystemV);
        let mut func = Function::with_name_signature(UserFuncName::default(), sig);
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut func, &mut builder_ctx);

        let resume_block = builder.create_block();
        switch_to_block_materialized(&mut builder, resume_block);

        let mut is_block_filled = false;
        materialize_label_block(&mut builder, resume_block, &mut is_block_filled);

        assert_eq!(builder.current_block(), Some(resume_block));
        assert!(
            !is_block_filled,
            "state_label materialization must leave the current resume block open"
        );
        assert!(
            builder.func.layout.last_inst(resume_block).is_none(),
            "state_label materialization must not emit a self-jump predecessor"
        );
    }

    // ── scan_loop_int_sum_reduction tests ──────────────────────────

    #[test]
    fn sum_reduction_detects_canonical_pattern() {
        // Simulates the IR for:
        //   total = 0
        //   for x in list_of_ints:
        //       total += x
        let ops = vec![
            list_int_new("my_list"),
            // 0: loop_start
            OpIR {
                kind: "loop_start".to_string(),
                ..OpIR::default()
            },
            // 1: loop_index_start  (idx)
            OpIR {
                kind: "loop_index_start".to_string(),
                out: Some("idx".to_string()),
                args: Some(vec!["start_val".to_string()]),
                ..OpIR::default()
            },
            // 2: index  list[idx]  -> elem
            OpIR {
                kind: "index".to_string(),
                args: Some(vec!["my_list".to_string(), "idx".to_string()]),
                out: Some("elem".to_string()),
                bce_safe: Some(true),
                ..OpIR::default()
            },
            // 3: add  [total, elem]  -> sum_result
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["total".to_string(), "elem".to_string()]),
                out: Some("sum_result".to_string()),
                ..OpIR::default()
            },
            // 4: store_var  total = sum_result
            OpIR {
                kind: "store_var".to_string(),
                var: Some("total".to_string()),
                args: Some(vec!["sum_result".to_string()]),
                ..OpIR::default()
            },
            // 5: loop_index_next
            OpIR {
                kind: "loop_index_next".to_string(),
                args: Some(vec!["next_idx".to_string()]),
                out: Some("idx_next".to_string()),
                ..OpIR::default()
            },
            // 6: loop_continue
            OpIR {
                kind: "loop_continue".to_string(),
                ..OpIR::default()
            },
            // 7: loop_end
            OpIR {
                kind: "loop_end".to_string(),
                ..OpIR::default()
            },
        ];

        let plan = representation_plan_for_ops(&ops);
        let result = scan_loop_int_sum_reduction(&ops, 2, "idx", &plan);
        assert!(result.is_some(), "canonical sum reduction must be detected");
        let candidate = result.unwrap();
        assert_eq!(candidate.list_name, "my_list");
        assert_eq!(candidate.acc_store_slot, "total");
        assert_eq!(candidate.add_out_name, "sum_result");
        assert_eq!(candidate.acc_operand_name, "total");
        assert_eq!(candidate.loop_end_idx, 8);
    }

    #[test]
    fn sum_reduction_detects_reversed_add_operands() {
        // add [elem, total] instead of [total, elem]
        let ops = vec![
            list_int_new("lst"),
            OpIR {
                kind: "loop_index_start".to_string(),
                out: Some("i".to_string()),
                args: Some(vec!["zero".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "index".to_string(),
                args: Some(vec!["lst".to_string(), "i".to_string()]),
                out: Some("e".to_string()),
                bce_safe: Some(true),
                ..OpIR::default()
            },
            OpIR {
                kind: "inplace_add".to_string(),
                args: Some(vec!["e".to_string(), "acc".to_string()]),
                out: Some("new_acc".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("acc".to_string()),
                args: Some(vec!["new_acc".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".to_string(),
                ..OpIR::default()
            },
        ];

        let plan = representation_plan_for_ops(&ops);
        let result = scan_loop_int_sum_reduction(&ops, 1, "i", &plan);
        assert!(
            result.is_some(),
            "reversed operand sum reduction must be detected"
        );
        let c = result.unwrap();
        assert_eq!(c.acc_operand_name, "acc");
        assert_eq!(c.list_name, "lst");
    }

    #[test]
    fn sum_reduction_rejects_non_bce_safe() {
        let ops = vec![
            list_int_new("lst"),
            OpIR {
                kind: "loop_index_start".to_string(),
                out: Some("i".to_string()),
                args: Some(vec!["zero".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "index".to_string(),
                args: Some(vec!["lst".to_string(), "i".to_string()]),
                out: Some("e".to_string()),
                bce_safe: None, // NOT bce_safe
                ..OpIR::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["acc".to_string(), "e".to_string()]),
                out: Some("new_acc".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("acc".to_string()),
                args: Some(vec!["new_acc".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".to_string(),
                ..OpIR::default()
            },
        ];
        let plan = representation_plan_for_ops(&ops);

        assert!(
            scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
            "non-bce_safe index must disqualify sum reduction"
        );
    }

    #[test]
    fn sum_reduction_rejects_call_in_body() {
        let ops = vec![
            list_int_new("lst"),
            OpIR {
                kind: "loop_index_start".to_string(),
                out: Some("i".to_string()),
                args: Some(vec!["zero".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "index".to_string(),
                args: Some(vec!["lst".to_string(), "i".to_string()]),
                out: Some("e".to_string()),
                bce_safe: Some(true),
                ..OpIR::default()
            },
            // Side-effecting call in loop body — disqualifies
            OpIR {
                kind: "call".to_string(),
                args: Some(vec!["e".to_string()]),
                out: Some("result".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["acc".to_string(), "result".to_string()]),
                out: Some("new_acc".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("acc".to_string()),
                args: Some(vec!["new_acc".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".to_string(),
                ..OpIR::default()
            },
        ];
        let plan = representation_plan_for_ops(&ops);

        assert!(
            scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
            "call in loop body must disqualify sum reduction"
        );
    }

    #[test]
    fn sum_reduction_rejects_nested_loop() {
        let ops = vec![
            list_int_new("lst"),
            OpIR {
                kind: "loop_index_start".to_string(),
                out: Some("i".to_string()),
                args: Some(vec!["zero".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "index".to_string(),
                args: Some(vec!["lst".to_string(), "i".to_string()]),
                out: Some("e".to_string()),
                bce_safe: Some(true),
                ..OpIR::default()
            },
            // Nested loop
            OpIR {
                kind: "loop_start".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".to_string(),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["acc".to_string(), "e".to_string()]),
                out: Some("new_acc".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("acc".to_string()),
                args: Some(vec!["new_acc".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".to_string(),
                ..OpIR::default()
            },
        ];
        let plan = representation_plan_for_ops(&ops);

        assert!(
            scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
            "nested loop must disqualify sum reduction"
        );
    }

    #[test]
    fn sum_reduction_rejects_wrong_index_var() {
        // Index uses a different variable than the loop induction variable
        let ops = vec![
            list_int_new("lst"),
            OpIR {
                kind: "loop_index_start".to_string(),
                out: Some("i".to_string()),
                args: Some(vec!["zero".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "index".to_string(),
                args: Some(vec!["lst".to_string(), "other_var".to_string()]),
                out: Some("e".to_string()),
                bce_safe: Some(true),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["acc".to_string(), "e".to_string()]),
                out: Some("new_acc".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("acc".to_string()),
                args: Some(vec!["new_acc".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".to_string(),
                ..OpIR::default()
            },
        ];
        let plan = representation_plan_for_ops(&ops);

        assert!(
            scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
            "index with non-induction variable must disqualify"
        );
    }

    #[test]
    fn sum_reduction_rejects_non_list_int() {
        let ops = vec![
            OpIR {
                kind: "loop_index_start".to_string(),
                out: Some("i".to_string()),
                args: Some(vec!["zero".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "index".to_string(),
                args: Some(vec!["lst".to_string(), "i".to_string()]),
                out: Some("e".to_string()),
                container_type: Some("list".to_string()), // generic list, not list_int
                bce_safe: Some(true),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["acc".to_string(), "e".to_string()]),
                out: Some("new_acc".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("acc".to_string()),
                args: Some(vec!["new_acc".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".to_string(),
                ..OpIR::default()
            },
        ];
        let plan = representation_plan_for_ops(&ops);

        assert!(
            scan_loop_int_sum_reduction(&ops, 0, "i", &plan).is_none(),
            "non-list_int container must disqualify"
        );
    }

    #[test]
    fn sum_reduction_rejects_multiple_stores() {
        let ops = vec![
            list_int_new("lst"),
            OpIR {
                kind: "loop_index_start".to_string(),
                out: Some("i".to_string()),
                args: Some(vec!["zero".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "index".to_string(),
                args: Some(vec!["lst".to_string(), "i".to_string()]),
                out: Some("e".to_string()),
                bce_safe: Some(true),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["acc".to_string(), "e".to_string()]),
                out: Some("new_acc".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("acc".to_string()),
                args: Some(vec!["new_acc".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("other".to_string()),
                args: Some(vec!["e".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".to_string(),
                ..OpIR::default()
            },
        ];
        let plan = representation_plan_for_ops(&ops);

        assert!(
            scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
            "multiple store_var ops must disqualify"
        );
    }

    #[test]
    fn sum_reduction_rejects_add_elem_mismatch() {
        // add operands don't include the index element
        let ops = vec![
            list_int_new("lst"),
            OpIR {
                kind: "loop_index_start".to_string(),
                out: Some("i".to_string()),
                args: Some(vec!["zero".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "index".to_string(),
                args: Some(vec!["lst".to_string(), "i".to_string()]),
                out: Some("e".to_string()),
                bce_safe: Some(true),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["acc".to_string(), "other_val".to_string()]),
                out: Some("new_acc".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("acc".to_string()),
                args: Some(vec!["new_acc".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "loop_end".to_string(),
                ..OpIR::default()
            },
        ];
        let plan = representation_plan_for_ops(&ops);

        assert!(
            scan_loop_int_sum_reduction(&ops, 1, "i", &plan).is_none(),
            "add operand mismatch must disqualify"
        );
    }

    // ── scalar_slot_exclusion_unsafe tests ──────────────────────────

    #[test]
    fn slot_exclusion_marks_call_arg_as_unsafe() {
        let func = FunctionIR {
            name: "call_escape".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("x".to_string()),
                    value: Some(42),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call".to_string(),
                    args: Some(vec!["x".to_string()]),
                    out: Some("result".to_string()),
                    s_value: Some("some_fn".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());
        assert!(
            analysis.scalar_slot_exclusion_unsafe.contains("x"),
            "int variable passed to call must be marked unsafe for slot exclusion"
        );
    }

    #[test]
    fn slot_exclusion_marks_returned_var_as_unsafe() {
        let func = FunctionIR {
            name: "ret_escape".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("x".to_string()),
                    value: Some(7),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("x".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());
        assert!(
            analysis.scalar_slot_exclusion_unsafe.contains("x"),
            "int variable in ret must be marked unsafe for slot exclusion"
        );
    }

    #[test]
    fn slot_exclusion_marks_store_attr_value_as_unsafe() {
        let func = FunctionIR {
            name: "heap_escape".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("val".to_string()),
                    value: Some(99),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_attr".to_string(),
                    args: Some(vec!["obj".to_string(), "val".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());
        assert!(
            analysis.scalar_slot_exclusion_unsafe.contains("val"),
            "int variable in store_attr must be marked unsafe for slot exclusion"
        );
    }

    #[test]
    fn slot_exclusion_marks_refcount_ops_as_unsafe() {
        let func = FunctionIR {
            name: "refcount_escape".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("x".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "inc_ref".to_string(),
                    args: Some(vec!["x".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());
        assert!(
            analysis.scalar_slot_exclusion_unsafe.contains("x"),
            "int variable with inc_ref must be marked unsafe for slot exclusion"
        );
    }

    #[test]
    fn slot_exclusion_marks_refcount_var_field_as_unsafe() {
        // A dec_ref op that references a scalar via op.var must also
        // mark it unsafe -- the runtime will dec_ref the boxed value
        // and needs the slot-backed refcount-correct representation.
        let func = FunctionIR {
            name: "refcount_var_escape".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("x".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "dec_ref".to_string(),
                    var: Some("x".to_string()),
                    args: Some(vec!["x".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());
        assert!(
            analysis.scalar_slot_exclusion_unsafe.contains("x"),
            "int variable in dec_ref var field must be marked unsafe for slot exclusion"
        );
    }

    #[test]
    fn slot_exclusion_marks_release_var_field_as_unsafe() {
        // release op referencing a scalar via op.var
        let func = FunctionIR {
            name: "release_var_escape".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("y".to_string()),
                    value: Some(42),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "release".to_string(),
                    var: Some("y".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());
        assert!(
            analysis.scalar_slot_exclusion_unsafe.contains("y"),
            "int variable in release var field must be marked unsafe for slot exclusion"
        );
    }

    #[test]
    fn slot_exclusion_safe_for_pure_arithmetic_loop() {
        // Pure arithmetic: x = const, loop { x += 1 } -- no escape
        let func = FunctionIR {
            name: "safe_arith".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("x".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("_bb1_arg0".to_string()),
                    args: Some(vec!["x".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_start".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".to_string(),
                    var: Some("_bb1_arg0".to_string()),
                    out: Some("cur".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("one".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "inplace_add".to_string(),
                    args: Some(vec!["cur".to_string(), "one".to_string()]),
                    out: Some("next".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("_bb1_arg0".to_string()),
                    args: Some(vec!["next".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_continue".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());
        assert!(
            !analysis.scalar_slot_exclusion_unsafe.contains("x"),
            "pure arithmetic loop var must NOT be marked unsafe"
        );
        assert!(
            !analysis.scalar_slot_exclusion_unsafe.contains("_bb1_arg0"),
            "join slot for pure arithmetic loop must NOT be marked unsafe"
        );
        assert!(
            !analysis.scalar_slot_exclusion_unsafe.contains("cur"),
            "loaded loop var must NOT be marked unsafe"
        );
    }

    #[test]
    fn slot_exclusion_marks_store_index_on_generic_list() {
        // Storing int to a generic list requires boxing correctness
        let func = FunctionIR {
            name: "list_store_escape".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("idx".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("val".to_string()),
                    value: Some(42),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_index".to_string(),
                    args: Some(vec![
                        "lst".to_string(),
                        "idx".to_string(),
                        "val".to_string(),
                    ]),
                    container_type: Some("list".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());
        assert!(
            analysis.scalar_slot_exclusion_unsafe.contains("val"),
            "int value stored to generic list must be marked unsafe"
        );
    }

    #[test]
    fn slot_exclusion_allows_store_index_on_list_int() {
        // Storing int to list_int is safe (flat i64 storage, no boxing)
        let func = FunctionIR {
            name: "list_int_store_safe".to_string(),
            params: vec![],
            ops: vec![
                list_int_new("lst"),
                OpIR {
                    kind: "const".to_string(),
                    out: Some("idx".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("val".to_string()),
                    value: Some(42),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_index".to_string(),
                    args: Some(vec![
                        "lst".to_string(),
                        "idx".to_string(),
                        "val".to_string(),
                    ]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let analysis = preanalyze_for_test(&func, &BTreeMap::new());
        assert!(
            !analysis.scalar_slot_exclusion_unsafe.contains("val"),
            "int value stored to list_int must NOT be marked unsafe"
        );
    }
}
