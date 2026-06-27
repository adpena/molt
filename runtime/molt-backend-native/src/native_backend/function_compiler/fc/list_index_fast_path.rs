use super::super::*;

/// Shared list/index fast-path state for native codegen.
///
/// These Cranelift Variables cache list storage facts across loop iterations
/// through SSA phis. Mutating list ops must invalidate the list's cached data,
/// length, and element-kind variables through this authority instead of editing
/// each map independently.
#[cfg(feature = "native-backend")]
#[derive(Default)]
pub(in crate::native_backend::function_compiler) struct ListIndexFastPathState {
    pub(in crate::native_backend::function_compiler) list_int_data_cache:
        BTreeMap<String, Variable>,
    pub(in crate::native_backend::function_compiler) list_int_len_cache: BTreeMap<String, Variable>,
    pub(in crate::native_backend::function_compiler) list_data_cache: BTreeMap<String, Variable>,
    pub(in crate::native_backend::function_compiler) list_len_cache: BTreeMap<String, Variable>,
    pub(in crate::native_backend::function_compiler) list_is_bool_cache: BTreeMap<String, Variable>,
    pub(in crate::native_backend::function_compiler) conditional_list_bool_shadows:
        BTreeMap<String, ConditionalListBoolShadow>,
}

#[cfg(feature = "native-backend")]
impl ListIndexFastPathState {
    pub(in crate::native_backend::function_compiler) fn invalidate_for_list_mutation(
        &mut self,
        list_name: &str,
    ) {
        self.list_int_data_cache.remove(list_name);
        self.list_int_len_cache.remove(list_name);
        self.list_data_cache.remove(list_name);
        self.list_len_cache.remove(list_name);
        self.list_is_bool_cache.remove(list_name);
        self.conditional_list_bool_shadows
            .retain(|_, shadow| shadow.list_name != list_name);
    }

    pub(in crate::native_backend::function_compiler) fn invalidate_for_store_index(
        &mut self,
        list_name: &str,
    ) {
        self.invalidate_for_list_mutation(list_name);
    }
}
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn loop_start_has_index_prelude(
    ops: &[OpIR],
    start_idx: usize,
) -> bool {
    let mut scan_idx = start_idx + 1;
    while let Some(next) = ops.get(scan_idx) {
        let kind = next.kind.as_str();
        if kind == "loop_index_start" {
            return true;
        }
        if kind.starts_with("const") {
            scan_idx += 1;
            continue;
        }
        return false;
    }
    false
}

/// Scan a loop body (from `start_idx+1` to the matching `loop_end`) and return
/// the set of list variable names whose data_ptr/len can be hoisted before the
/// loop.  A variable is hoistable when it is read through `index` and NOT
/// mutated by `store_index`, `list_append`, `list_pop`, `list_extend`,
/// `list_insert`, `list_remove`, or `list_clear` anywhere in the loop body.
///
/// Returns `(list_int_hoistable, list_generic_hoistable)`.
///
/// Hoisting requires that the list's SSA name be **defined before the loop
/// header** so its NaN-boxed pointer is available in the pre-loop block.
/// Variables defined inside the loop body are filtered out — hoisting them
/// would emit `obj_ptr = use_var(undef) = 0` followed by a NULL header read,
/// which traps at runtime.  This preserves correctness for loops that index
/// through an outer-scope value (e.g. boxed-cell reads of `list[i]` inside a
/// list comprehension whose enclosing function preboxed the local).
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn scan_loop_hoistable_lists(
    ops: &[OpIR],
    start_idx: usize,
    pre_loop_defined: &BTreeSet<String>,
    representation_plan: &ScalarRepresentationPlan,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut list_int_accessed: BTreeSet<String> = BTreeSet::new();
    let mut list_generic_accessed: BTreeSet<String> = BTreeSet::new();
    let mut mutated: BTreeSet<String> = BTreeSet::new();

    let mut depth = 0i32;
    for idx in (start_idx + 1)..ops.len() {
        let op = &ops[idx];
        match op.kind.as_str() {
            "loop_start" | "loop_index_start" => depth += 1,
            "loop_end" if depth > 0 => depth -= 1,
            "loop_end" => break,
            _ => {}
        }
        // Only consider ops at the current loop nesting level (depth == 0).
        // Inner loop accesses get their own hoisting when their loop_start is
        // processed.
        if depth != 0 {
            continue;
        }
        let args = match op.args.as_ref() {
            Some(a) if !a.is_empty() => a,
            _ => continue,
        };
        match op.kind.as_str() {
            "index" => {
                if representation_plan.op_has_container_storage(
                    idx,
                    op,
                    ContainerStorageKind::FlatListInt,
                ) {
                    list_int_accessed.insert(args[0].clone());
                } else if representation_plan.op_has_container_kind(op, ContainerKind::List) {
                    list_generic_accessed.insert(args[0].clone());
                }
            }
            "store_index" | "list_append" | "list_pop" | "list_extend" | "list_insert"
            | "list_remove" | "list_clear" => {
                mutated.insert(args[0].clone());
            }
            _ => {}
        }
    }
    list_int_accessed.retain(|v| !mutated.contains(v) && pre_loop_defined.contains(v));
    list_generic_accessed.retain(|v| !mutated.contains(v) && pre_loop_defined.contains(v));
    (list_int_accessed, list_generic_accessed)
}

/// Collect the set of SSA names defined by ops at indices `[0, start_idx)`.
/// Used to gate loop-invariant list pointer hoisting so we never hoist a
/// value that is produced inside the loop body (which would emit
/// `use_var(undef) = 0` and trap on the subsequent header load).
///
/// Function parameters are added by the caller via the param iterator;
/// this routine only walks `ops`.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn collect_pre_loop_defined_names(
    ops: &[OpIR],
    start_idx: usize,
) -> BTreeSet<String> {
    let mut defined: BTreeSet<String> = BTreeSet::new();
    for op in ops.iter().take(start_idx) {
        if let Some(out) = op.out.as_ref() {
            defined.insert(out.clone());
        }
        if let Some(var) = op.var.as_ref() {
            defined.insert(var.clone());
        }
    }
    defined
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn generic_list_int_lane_eligible(
    representation_plan: &ScalarRepresentationPlan,
    op: &OpIR,
    integer_key_lane: bool,
) -> bool {
    integer_key_lane && representation_plan.op_has_container_kind(op, ContainerKind::List)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn emit_set_contains_refs_if_heap(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    list_bits: Value,
    val_bits: Value,
) {
    let tag_bits = builder.ins().band_imm(val_bits, (QNAN | TAG_MASK) as i64);
    let is_ptr = builder
        .ins()
        .icmp_imm(IntCC::Equal, tag_bits, (QNAN | TAG_PTR) as i64);
    let set_flag_block = builder.create_block();
    let done_block = builder.create_block();
    builder
        .ins()
        .brif(is_ptr, set_flag_block, &[], done_block, &[]);

    switch_to_block_materialized(builder, set_flag_block);
    seal_block_once(builder, sealed_blocks, set_flag_block);
    let masked = builder.ins().band_imm(list_bits, POINTER_MASK as i64);
    let shifted = builder.ins().ishl_imm(masked, 16);
    let obj_ptr = builder.ins().sshr_imm(shifted, 16);
    let flags = builder.ins().load(
        types::I32,
        MemFlagsData::trusted(),
        obj_ptr,
        HEADER_FLAGS_OFFSET,
    );
    let contains_refs = builder
        .ins()
        .iconst(types::I32, i64::from(HEADER_FLAG_CONTAINS_REFS));
    let flags = builder.ins().bor(flags, contains_refs);
    builder
        .ins()
        .store(MemFlagsData::trusted(), flags, obj_ptr, HEADER_FLAGS_OFFSET);
    jump_block(builder, done_block, &[]);

    switch_to_block_materialized(builder, done_block);
    seal_block_once(builder, sealed_blocks, done_block);
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn emit_regular_list_container_absorb_store(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    list_bits: Value,
    elem_addr: Value,
    val_bits: Value,
    val_known_non_heap: bool,
    local_inc_ref_obj: FuncRef,
    local_dec_ref_obj: FuncRef,
    nbc: &crate::NanBoxConsts,
    merge_block: Block,
) {
    let old_elem = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), elem_addr, 0);
    let same_elem = builder.ins().icmp(IntCC::Equal, old_elem, val_bits);
    let same_block = builder.create_block();
    let replace_block = builder.create_block();
    builder
        .ins()
        .brif(same_elem, same_block, &[], replace_block, &[]);

    switch_to_block_materialized(builder, same_block);
    seal_block_once(builder, sealed_blocks, same_block);
    jump_block(builder, merge_block, &[]);

    switch_to_block_materialized(builder, replace_block);
    seal_block_once(builder, sealed_blocks, replace_block);
    if !val_known_non_heap {
        emit_inc_ref_obj(builder, val_bits, local_inc_ref_obj, nbc);
        emit_set_contains_refs_if_heap(builder, sealed_blocks, list_bits, val_bits);
    }
    builder
        .ins()
        .store(MemFlagsData::trusted(), val_bits, elem_addr, 0);
    emit_dec_ref_obj(builder, old_elem, local_dec_ref_obj, nbc);
    jump_block(builder, merge_block, &[]);
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn index_fallback_import_name(
    representation_plan: &ScalarRepresentationPlan,
    op: &OpIR,
    integer_key_lane: bool,
) -> &'static str {
    match representation_plan.op_container_kind(op) {
        Some(ContainerKind::Dict) => "molt_dict_getitem",
        Some(ContainerKind::Tuple) => "molt_tuple_getitem",
        _ if integer_key_lane => "molt_list_getitem_int_fast",
        _ => "molt_index",
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn store_index_fallback_import_name(
    representation_plan: &ScalarRepresentationPlan,
    op: &OpIR,
    integer_key_lane: bool,
) -> &'static str {
    match representation_plan.op_container_kind(op) {
        Some(ContainerKind::Dict) => "molt_dict_setitem",
        _ if integer_key_lane => "molt_list_setitem_int_fast",
        _ => "molt_store_index",
    }
}

/// Describes a recognized integer sum-reduction loop eligible for 4x unrolling.
///
/// Pattern:
/// ```text
/// loop_index_start  (idx)
///   ...
///   index  list_name[idx]  FlatListInt storage proof  bce_safe=true  -> elem
///   add/inplace_add  [acc, elem] -> acc_next   (or [elem, acc])
///   store_var  acc_slot = acc_next
///   ...
///   loop_index_next
///   loop_continue / loop_end
/// ```
///
/// When detected, the native backend emits a 4x-unrolled main loop
/// (4 scalar loads + 4 scalar adds per iteration, index advances by 4)
/// followed by a scalar epilogue for the remaining 0-3 elements.
/// This reduces loop overhead (branch, compare, increment) by 4x.
#[cfg(feature = "native-backend")]
#[derive(Debug, Clone)]
pub(in crate::native_backend::function_compiler) struct SumReductionCandidate {
    /// The list variable name being iterated.
    pub(in crate::native_backend::function_compiler) list_name: String,
    /// The accumulator variable name (the store_var target).
    pub(in crate::native_backend::function_compiler) acc_store_slot: String,
    /// The add/inplace_add output name (feeds into the store_var).
    pub(in crate::native_backend::function_compiler) add_out_name: String,
    /// The element variable name (output of the index op).
    /// Retained for diagnostic/debug logging; not consumed by codegen.
    #[allow(dead_code)]
    pub(in crate::native_backend::function_compiler) elem_name: String,
    /// The accumulator operand name in the add op (the other operand besides elem).
    pub(in crate::native_backend::function_compiler) acc_operand_name: String,
    /// Op index of the loop_end (exclusive bound for skipping body ops).
    pub(in crate::native_backend::function_compiler) loop_end_idx: usize,
}

/// Scan the loop body from `loop_index_start_idx` to the matching `loop_end`
/// and detect a simple integer sum-reduction pattern over a `list_int`.
///
/// Returns `Some(candidate)` only when ALL of the following hold:
///   1. The loop body contains exactly one `index` op with a shared
///      `FlatListInt` storage proof and `bce_safe=true`.
///   2. The loop body contains exactly one `add` or `inplace_add` op whose operands
///      include the element from (1) and an accumulator, and whose output feeds
///      into a single `store_var`.
///   3. No other side-effecting ops exist in the body (calls, other stores, etc.).
///   4. The loop is not nested (no inner `loop_start`/`loop_index_start`).
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn scan_loop_int_sum_reduction(
    ops: &[OpIR],
    loop_index_start_idx: usize,
    index_var_name: &str,
    representation_plan: &ScalarRepresentationPlan,
) -> Option<SumReductionCandidate> {
    // Find the matching loop_end.
    let mut depth = 0i32;
    let mut loop_end_idx = None;
    for i in (loop_index_start_idx + 1)..ops.len() {
        match ops[i].kind.as_str() {
            "loop_start" | "loop_index_start" => depth += 1,
            "loop_end" if depth > 0 => depth -= 1,
            "loop_end" => {
                loop_end_idx = Some(i);
                break;
            }
            _ => {}
        }
    }
    let loop_end_idx = loop_end_idx?;

    // Scan the body for the pattern components.
    let mut index_op: Option<(usize, String, String)> = None; // (idx, list_name, elem_out)
    let mut add_op: Option<(String, String, String)> = None; // (acc_operand, elem_operand, add_out)
    let mut store_var_op: Option<(String, String)> = None; // (slot_name, source_name)
    let mut has_nested_loop = false;
    let mut has_side_effects = false;

    for i in (loop_index_start_idx + 1)..loop_end_idx {
        let op = &ops[i];
        match op.kind.as_str() {
            "loop_start" | "loop_index_start" => {
                has_nested_loop = true;
                break;
            }
            "index" => {
                if !representation_plan.op_has_container_storage(
                    i,
                    op,
                    ContainerStorageKind::FlatListInt,
                ) {
                    return None; // non-flat-list-int storage disqualifies
                }
                if op.bce_safe != Some(true) {
                    return None; // bounds check needed — can't safely unroll
                }
                if index_op.is_some() {
                    return None; // multiple index ops — too complex
                }
                let args = op.args.as_ref()?;
                if args.len() < 2 {
                    return None;
                }
                // args[0] = list name, args[1] = index var
                // The index must be the loop induction variable.
                if args[1] != index_var_name {
                    return None;
                }
                let out = op.out.as_ref()?;
                index_op = Some((i, args[0].clone(), out.clone()));
            }
            "add" | "inplace_add" => {
                if add_op.is_some() {
                    return None; // multiple adds — too complex
                }
                let args = op.args.as_ref()?;
                if args.len() < 2 {
                    return None;
                }
                let out = op.out.as_ref()?;
                add_op = Some((args[0].clone(), args[1].clone(), out.clone()));
            }
            "store_var" => {
                if store_var_op.is_some() {
                    return None; // multiple store_vars — too complex
                }
                let slot = op.var.as_ref()?;
                let args = op.args.as_ref()?;
                if args.is_empty() {
                    return None;
                }
                store_var_op = Some((slot.clone(), args[0].clone()));
            }
            // Structural ops that don't affect correctness:
            "loop_index_next"
            | "loop_continue"
            | "loop_break_if_true"
            | "loop_break_if_false"
            | "loop_break_if_exception"
            | "loop_break"
            | "const"
            | "const_bool"
            | "const_float"
            | "const_str"
            | "copy"
            | "copy_var"
            | "load_var"
            | "lt"
            | "le"
            | "gt"
            | "ge"
            | "not"
            | "line"
            | "label"
            | "phi" => {}
            // Anything else (calls, other stores, etc.) disqualifies.
            _ => {
                has_side_effects = true;
            }
        }
    }

    if has_nested_loop || has_side_effects {
        return None;
    }

    let (_, list_name, elem_name) = index_op?;
    let (add_arg0, add_arg1, add_out) = add_op?;
    let (store_slot, store_source) = store_var_op?;

    // The store_var must store the add output.
    if store_source != add_out {
        return None;
    }

    // One of the add operands must be the element, the other is the accumulator.
    let acc_operand = if add_arg0 == elem_name {
        add_arg1.clone()
    } else if add_arg1 == elem_name {
        add_arg0.clone()
    } else {
        return None; // neither add operand is the element
    };

    Some(SumReductionCandidate {
        list_name,
        acc_store_slot: store_slot,
        add_out_name: add_out,
        elem_name,
        acc_operand_name: acc_operand,
        loop_end_idx,
    })
}
