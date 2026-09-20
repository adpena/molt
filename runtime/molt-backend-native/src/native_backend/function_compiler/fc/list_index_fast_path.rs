use super::super::*;

/// Shared list/index fast-path state for native codegen.
///
/// Ordinary storage observations are block-local. Cross-block reuse is only
/// admitted inside a CFG/effect-certified loop, never by name presence alone.
#[cfg(feature = "native-backend")]
#[derive(Default)]
pub(in crate::native_backend::function_compiler) struct ListIndexFastPathState {
    storage: BTreeMap<String, BTreeMap<ListStorageField, ScopedListVariable>>,
    bool_shadows: BTreeMap<String, (Block, ConditionalListBoolShadow)>,
    current_op: usize,
    publishing_loop: Option<ListStorageLoopScope>,
    cleanup_generation: std::rc::Rc<std::cell::Cell<u64>>,
    // Immutable derivation of the function's canonical CFG, built only if a
    // loop survives effect filtering and shared by every hoist scan.
    loop_execution_dominators: Option<Vec<Option<usize>>>,
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::native_backend::function_compiler) enum ListStorageField {
    IntData,
    IntLen,
    Data,
    Len,
    IsBool,
}

#[cfg(feature = "native-backend")]
struct ScopedListVariable {
    variable: Variable,
    block: Block,
    loop_scope: Option<ListStorageLoopScope>,
    cleanup_generation: u64,
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Copy)]
struct ListStorageLoopScope {
    start: usize,
    end: usize,
    preheader: Block,
}

#[cfg(feature = "native-backend")]
impl ListIndexFastPathState {
    pub(in crate::native_backend::function_compiler) fn new(cleanup: &NativeCleanupRoots) -> Self {
        Self {
            cleanup_generation: cleanup.release_generation(),
            ..Self::default()
        }
    }

    pub(in crate::native_backend::function_compiler) fn begin_op(
        &mut self,
        op_idx: usize,
        op: &OpIR,
        plan: &ScalarRepresentationPlan,
    ) {
        self.current_op = op_idx;
        self.publishing_loop = None;
        if !self.storage.is_empty() && !list_storage_op_preserves_heap(op_idx, op, plan) {
            self.storage.clear();
        }
        let generation = self.cleanup_generation.get();
        self.storage.retain(|_, fields| {
            fields.retain(|_, cached| {
                cached.cleanup_generation == generation
                    && cached
                        .loop_scope
                        .is_none_or(|scope| scope.start <= op_idx && op_idx <= scope.end)
            });
            !fields.is_empty()
        });
        crate::tir::simple_def_use::visit_simple_ir_defined_names(op, |name| {
            self.storage.remove(name);
            self.bool_shadows.remove(name);
        });
    }

    pub(in crate::native_backend::function_compiler) fn get(
        &self,
        field: ListStorageField,
        name: &str,
        builder: &FunctionBuilder,
    ) -> Option<Variable> {
        let cached = self.storage.get(name)?.get(&field)?;
        let in_scope = cached.loop_scope.map_or_else(
            || builder.current_block() == Some(cached.block),
            |scope| {
                cached.block == scope.preheader
                    && scope.start <= self.current_op
                    && self.current_op <= scope.end
            },
        );
        (in_scope && cached.cleanup_generation == self.cleanup_generation.get())
            .then_some(cached.variable)
    }

    pub(in crate::native_backend::function_compiler) fn insert(
        &mut self,
        field: ListStorageField,
        name: String,
        variable: Variable,
        builder: &FunctionBuilder,
    ) {
        let block = builder
            .current_block()
            .expect("cached storage has a defining block");
        self.storage.entry(name).or_default().insert(
            field,
            ScopedListVariable {
                variable,
                block,
                // Only the actual certified preheader may publish loop invariants.
                // An internal lowering split cannot inherit this privilege.
                loop_scope: self
                    .publishing_loop
                    .filter(|scope| scope.preheader == block),
                cleanup_generation: self.cleanup_generation.get(),
            },
        );
    }

    pub(in crate::native_backend::function_compiler) fn insert_bool_shadow(
        &mut self,
        name: String,
        shadow: ConditionalListBoolShadow,
        builder: &FunctionBuilder,
    ) {
        self.bool_shadows.insert(
            name,
            (
                builder
                    .current_block()
                    .expect("list bool shadow has a defining block"),
                shadow,
            ),
        );
    }

    pub(in crate::native_backend::function_compiler) fn bool_shadow(
        &self,
        name: &str,
        builder: &FunctionBuilder,
    ) -> Option<&ConditionalListBoolShadow> {
        let (block, shadow) = self.bool_shadows.get(name)?;
        (builder.current_block() == Some(*block)).then_some(shadow)
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

#[cfg(feature = "native-backend")]
enum ListIndexLayout {
    FlatInt,
    Generic,
}

#[cfg(feature = "native-backend")]
fn typed_list_index_layout(
    index: usize,
    op: &OpIR,
    plan: &ScalarRepresentationPlan,
) -> Option<ListIndexLayout> {
    if op.kind != "index"
        || !op
            .args
            .as_ref()
            .is_some_and(|args| args.len() == 2 && plan.name_is_integer_scalar(&args[1]))
    {
        return None;
    }
    if plan.op_has_container_storage(index, op, ContainerStorageKind::FlatListInt) {
        Some(ListIndexLayout::FlatInt)
    } else if plan.op_has_container_kind(op, ContainerKind::List) {
        Some(ListIndexLayout::Generic)
    } else {
        None
    }
}

/// One effect admission decision for both loop lifetime certification and
/// ordinary op-boundary invalidation. Non-capture is not non-mutation.
#[cfg(feature = "native-backend")]
fn list_storage_op_preserves_heap(
    index: usize,
    op: &OpIR,
    plan: &ScalarRepresentationPlan,
) -> bool {
    use crate::tir::op_kinds_generated::{
        copy_kind_is_explicit_no_heap_move_table, copy_kind_mints_owned_alias_ref_table,
        kind_to_opcode_table, opcode_effects_table, simpleir_kind_is_cfg_or_ssa_consumed,
        simpleir_kind_is_conditional_branch,
    };
    use crate::tir::ops::OpCode;
    use crate::tir::types::TirType;

    if op.is_async_work_poll() {
        return false;
    }
    if typed_list_index_layout(index, op, plan).is_some() {
        return true;
    }
    let mut operand_types = Vec::new();
    crate::tir::simple_def_use::visit_simple_ir_reads(op, |read| {
        operand_types.push(match plan.name_scalar_kind(read.name) {
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
    let scalar_binding = simple_ir_binding(op).is_none_or(|binding| {
        plan.name_is_non_heap_scalar(binding.destination)
            && operand_types.iter().all(|ty| {
                matches!(
                    ty,
                    TirType::I64 | TirType::Bool | TirType::F64 | TirType::None
                )
            })
    });
    // Acquiring a credit is not releasing an owner. These instructions retain
    // their runtime ownership behavior; only list-buffer observation is admitted.
    if (copy_kind_is_explicit_no_heap_move_table(kind) && scalar_binding)
        || copy_kind_mints_owned_alias_ref_table(kind)
        || opcode == Some(OpCode::IncRef)
    {
        return true;
    }
    if opcode.is_none()
        && simpleir_kind_is_cfg_or_ssa_consumed(kind)
        && (!simpleir_kind_is_conditional_branch(kind)
            || operand_types.iter().all(|ty| {
                matches!(
                    ty,
                    TirType::I64 | TirType::Bool | TirType::F64 | TirType::Str | TirType::None
                )
            }))
    {
        return true;
    }
    opcode.is_some_and(|opcode| {
        let effects = molt_ir::tir::op_semantics::op_instance_facts(opcode, &operand_types)
            .map_or_else(|| opcode_effects_table(opcode), |facts| facts.effects);
        effects.effect_free && !effects.may_access_arbitrary_heap
    })
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
    preheader: Block,
    cleanup: Option<&NativeCleanupRoots>,
) -> (BTreeSet<String>, BTreeSet<String>) {
    fast_paths.publishing_loop = None;
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
        let mut may_finalize = false;
        crate::tir::simple_def_use::visit_simple_ir_defined_names(op, |name| {
            body_definitions.insert(name.to_string());
            // Native owner replacement may run last iteration's finalizer even
            // when this opcode itself is pure. TIR-owned drops stay explicit.
            // Without concrete custody, a heap result may own a finalizer.
            may_finalize |= cleanup.is_none_or(|roots| roots.contains(name))
                && !representation_plan.name_is_non_heap_scalar(name);
        });
        if may_finalize {
            return (BTreeSet::new(), BTreeSet::new());
        }
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
        if let Some(layout) = typed_list_index_layout(idx, op, representation_plan) {
            if depth == 0 {
                let name = op.args.as_ref().unwrap()[0].clone();
                match layout {
                    ListIndexLayout::FlatInt => {
                        list_int_accessed.insert(name);
                    }
                    ListIndexLayout::Generic => {
                        list_generic_accessed.insert(name);
                    }
                }
            }
        }
        if !list_storage_op_preserves_heap(idx, op, representation_plan) {
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
    // Block-local observations cannot silently become loop invariants. Only
    // existing certified outer-loop observations may satisfy these producers.
    fast_paths.storage.retain(|_, fields| {
        fields.retain(|_, cached| cached.loop_scope.is_some());
        !fields.is_empty()
    });
    fast_paths.current_op = start_idx;
    fast_paths.publishing_loop = Some(ListStorageLoopScope {
        start: start_idx,
        end: loop_end,
        preheader,
    });
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

/// The sole preheader producer for plain and indexed native loops. Scope
/// certification and all storage/layout observations must move together.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn emit_loop_list_storage_hoists(
    func_ir: &FunctionIR,
    op_idx: usize,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    fast_paths: &mut ListIndexFastPathState,
    cleanup_roots: &NativeCleanupRoots,
    nbc: &crate::NanBoxConsts,
) {
    let mut pre_loop_defined = collect_pre_loop_defined_names(&func_ir.ops, op_idx);
    pre_loop_defined.extend(
        func_ir
            .params
            .iter()
            .filter(|name| name.as_str() != "none")
            .cloned(),
    );
    let (flat, generic) = scan_loop_hoistable_lists(
        &func_ir.ops,
        op_idx,
        &pre_loop_defined,
        representation_plan,
        fast_paths,
        builder.current_block().expect("list hoist has a preheader"),
        Some(cleanup_roots),
    );
    for (names, layout) in [
        (flat, ListIndexLayout::FlatInt),
        (generic, ListIndexLayout::Generic),
    ] {
        let (data_field, len_field) = match layout {
            ListIndexLayout::FlatInt => (ListStorageField::IntData, ListStorageField::IntLen),
            ListIndexLayout::Generic => (ListStorageField::Data, ListStorageField::Len),
        };
        for name in names {
            if fast_paths.get(data_field, &name, builder).is_some() {
                continue; // A certified outer loop already owns this observation.
            }
            let Some(obj) = super::var_get_boxed_overflow_safe_fn(
                module,
                import_ids,
                builder,
                import_refs,
                sealed_blocks,
                vars,
                &name,
                representation_plan,
                nbc,
            ) else {
                continue;
            };
            let masked = builder.ins().band_imm(*obj, POINTER_MASK as i64);
            let shifted = builder.ins().ishl_imm(masked, 16);
            let obj_ptr = builder.ins().sshr_imm(shifted, 16);
            let storage_ptr = builder
                .ins()
                .load(types::I64, MemFlagsData::trusted(), obj_ptr, 0);
            let (data, len) = match layout {
                ListIndexLayout::FlatInt => (
                    builder.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        storage_ptr,
                        LIST_INT_STORAGE_DATA_OFFSET,
                    ),
                    builder.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        storage_ptr,
                        LIST_INT_STORAGE_LEN_OFFSET,
                    ),
                ),
                ListIndexLayout::Generic => {
                    let tid = builder.ins().load(
                        types::I32,
                        MemFlagsData::trusted(),
                        obj_ptr,
                        HEADER_TYPE_ID_OFFSET,
                    );
                    let bool_tid = builder.ins().iconst(types::I32, JIT_TYPE_ID_LIST_BOOL);
                    let is_bool = builder.ins().icmp(IntCC::Equal, tid, bool_tid);
                    let layout_var = builder.declare_var(types::I8);
                    builder.def_var(layout_var, is_bool);
                    fast_paths.insert(ListStorageField::IsBool, name.clone(), layout_var, builder);
                    let vec_layout = vec_u64_layout();
                    // ListBoolStorage is repr(C); Vec<u64> uses probed offsets.
                    let bool_data =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), storage_ptr, 0);
                    let bool_len =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), storage_ptr, 8);
                    let vec_data = builder.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        storage_ptr,
                        vec_layout.data_offset,
                    );
                    let vec_len = builder.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        storage_ptr,
                        vec_layout.len_offset,
                    );
                    (
                        builder.ins().select(is_bool, bool_data, vec_data),
                        builder.ins().select(is_bool, bool_len, vec_len),
                    )
                }
            };
            let data_var = builder.declare_var(types::I64);
            builder.def_var(data_var, data);
            fast_paths.insert(data_field, name.clone(), data_var, builder);
            let len_var = builder.declare_var(types::I64);
            builder.def_var(len_var, len);
            fast_paths.insert(len_field, name, len_var, builder);
        }
    }
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
