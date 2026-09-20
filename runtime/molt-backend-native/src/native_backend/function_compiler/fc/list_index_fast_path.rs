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
    // Immutable derivation of the function's canonical CFG, built only if a
    // loop survives effect filtering and shared by every hoist scan.
    loop_execution_dominators: Option<Vec<Option<usize>>>,
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
/// loop. A variable is hoistable when typed indexing cannot invoke Python and
/// no operation at any nesting depth can invalidate the cached storage. Opaque
/// effects fence the whole heap: even a zero-argument call can mutate a list
/// through a global or captured alias.
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
    fast_paths: &mut ListIndexFastPathState,
) -> (BTreeSet<String>, BTreeSet<String>) {
    use crate::tir::op_kinds_generated::{
        copy_kind_is_explicit_no_heap_move_table, copy_kind_mints_owned_alias_ref_table,
        kind_to_opcode_table, opcode_effects_table, simpleir_kind_is_cfg_or_ssa_consumed,
        simpleir_kind_is_conditional_branch,
    };
    use crate::tir::ops::OpCode;
    use crate::tir::types::TirType;

    let mut list_int_accessed: BTreeSet<String> = BTreeSet::new();
    let mut list_generic_accessed: BTreeSet<String> = BTreeSet::new();
    let mut body_definitions = BTreeSet::new();
    let mut loop_end = None;
    // The loop_start before an indexed prelude is metadata for the same loop,
    // not an additional nested loop. This matches native loop emission.
    let mut body_start = start_idx;
    if ops.get(start_idx).is_some_and(|op| op.kind == "loop_start")
        && loop_start_has_index_prelude(ops, start_idx)
    {
        while ops[body_start].kind != "loop_index_start" {
            body_start += 1;
        }
    }
    let mut depth = 0i32;
    for idx in (body_start + 1)..ops.len() {
        let op = &ops[idx];
        crate::tir::simple_def_use::visit_simple_ir_defined_names(op, |name| {
            body_definitions.insert(name.to_string());
        });
        match op.kind.as_str() {
            "loop_start" if loop_start_has_index_prelude(ops, idx) => continue,
            "loop_start" | "loop_index_start" => {
                depth += 1;
                continue;
            }
            "loop_end" if depth > 0 => {
                depth -= 1;
                continue;
            }
            "loop_end" => {
                loop_end = Some(idx);
                break;
            }
            _ => {}
        }
        if op.is_async_work_poll() {
            return (BTreeSet::new(), BTreeSet::new());
        }
        let flat_index = op.kind == "index"
            && representation_plan.op_has_container_storage(
                idx,
                op,
                ContainerStorageKind::FlatListInt,
            );
        let list_index = op.kind == "index"
            && representation_plan.op_has_container_kind(op, ContainerKind::List);
        if (flat_index || list_index)
            && op.args.as_ref().is_some_and(|args| {
                args.len() == 2 && representation_plan.name_is_integer_scalar(&args[1])
            })
        {
            // Only current-depth accesses become candidates, but nested reads
            // still need this same no-callback proof before crossing the fence.
            if depth == 0 {
                let name = op.args.as_ref().unwrap()[0].clone();
                if flat_index {
                    list_int_accessed.insert(name);
                } else {
                    list_generic_accessed.insert(name);
                }
            }
            continue;
        }
        let mut operand_types = Vec::new();
        crate::tir::simple_def_use::visit_simple_ir_reads(op, |read| {
            operand_types.push(match representation_plan.name_scalar_kind(read.name) {
                Some(ScalarKind::Int) => TirType::I64,
                Some(ScalarKind::Bool) => TirType::Bool,
                Some(ScalarKind::Float) => TirType::F64,
                Some(ScalarKind::Str) => TirType::Str,
                Some(ScalarKind::NoneValue) => TirType::None,
                None => TirType::DynBox,
            });
        });
        let kind = op.kind.as_str();
        let opcode = kind_to_opcode_table(kind);
        // Reference acquisition and no-heap transport do not mutate list
        // storage. They remain real instructions with their own owner credits.
        // A binding replacement can run an old owner's finalizer, so it only
        // qualifies when both old home and incoming value are proven scalars.
        let scalar_binding = simple_ir_binding(op).is_none_or(|binding| {
            representation_plan.name_is_non_heap_scalar(binding.destination)
                && operand_types.iter().all(|ty| {
                    matches!(
                        ty,
                        TirType::I64 | TirType::Bool | TirType::F64 | TirType::None
                    )
                })
        });
        if (copy_kind_is_explicit_no_heap_move_table(kind) && scalar_binding)
            || copy_kind_mints_owned_alias_ref_table(kind)
            || opcode == Some(OpCode::IncRef)
        {
            continue;
        }
        if opcode.is_none() && simpleir_kind_is_cfg_or_ssa_consumed(kind) {
            if !simpleir_kind_is_conditional_branch(kind)
                || operand_types.iter().all(|ty| {
                    matches!(
                        ty,
                        TirType::I64 | TirType::Bool | TirType::F64 | TirType::Str | TirType::None
                    )
                })
            {
                continue;
            }
        }
        let effects = opcode.map(|opcode| {
            crate::tir::op_semantics::op_instance_facts(opcode, &operand_types)
                .map_or_else(|| opcode_effects_table(opcode), |facts| facts.effects)
        });
        // Non-capture is not non-mutation. Unknown/Copy-lifted runtime ops and
        // opaque callbacks invalidate every candidate, including unpassed lists.
        if effects.is_none_or(|effects| effects.may_access_arbitrary_heap || !effects.effect_free) {
            return (BTreeSet::new(), BTreeSet::new());
        }
    }
    list_int_accessed.retain(|v| pre_loop_defined.contains(v) && !body_definitions.contains(v));
    list_generic_accessed.retain(|v| pre_loop_defined.contains(v) && !body_definitions.contains(v));
    let Some(loop_end) = loop_end else {
        return (BTreeSet::new(), BTreeSet::new());
    };
    if list_int_accessed.is_empty() && list_generic_accessed.is_empty() {
        return (list_int_accessed, list_generic_accessed);
    }
    // A lexical prefix is not a dominance proof. Reuse the canonical execution
    // graph, including exception/resume edges, before installing preheader data.
    let dominators = fast_paths
        .loop_execution_dominators
        .get_or_insert_with(|| crate::tir::cfg::CFG::build(ops).execution_op_dominators(ops));
    let dominates = |definition: usize, mut use_index: usize| {
        loop {
            if definition == use_index {
                return true;
            }
            let Some(parent) = dominators[use_index] else {
                return false;
            };
            use_index = parent;
        }
    };
    if (body_start != 0 && dominators[body_start].is_none())
        || ((body_start + 1)..=loop_end).any(|index| {
            // An end marker after an unconditional continue is lexical only.
            // The canonical graph leaves unreachable nodes without an idom;
            // real exception/resume entries remain reachable and must pass.
            dominators[index].is_some() && !dominates(body_start, index)
        })
    {
        return (BTreeSet::new(), BTreeSet::new());
    }
    let mut lexical_definitions = BTreeSet::new();
    let mut dominating_definitions = BTreeSet::new();
    for (index, op) in ops.iter().enumerate().take(start_idx) {
        crate::tir::simple_def_use::visit_simple_ir_defined_names(op, |name| {
            lexical_definitions.insert(name.to_string());
            if dominates(index, body_start) {
                dominating_definitions.insert(name.to_string());
            }
        });
    }
    let available = |name: &String| {
        // Names supplied only by the caller are entry parameters.
        !lexical_definitions.contains(name) || dominating_definitions.contains(name)
    };
    list_int_accessed.retain(available);
    list_generic_accessed.retain(available);
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
        crate::tir::simple_def_use::visit_simple_ir_defined_names(op, |name| {
            defined.insert(name.to_string());
        });
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
) -> &'static str {
    match representation_plan.op_container_kind(op) {
        Some(ContainerKind::Dict) => "molt_dict_setitem",
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
                let binding = simple_ir_binding(op)?;
                // The reduction rewrite replaces this store, but does not
                // materialize a separate source snapshot for an optional result.
                if binding.result.is_some() {
                    return None;
                }
                let slot = binding.destination;
                let args = op.args.as_ref()?;
                if args.is_empty() {
                    return None;
                }
                store_var_op = Some((slot.to_string(), args[0].clone()));
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
