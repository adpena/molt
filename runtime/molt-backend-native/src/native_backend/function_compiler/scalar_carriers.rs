use super::*;

// Scalar carrier, boxing, and transport-boundary helpers shared by the native
// function compiler shell and extracted fc::* op families. Keep this module
// private to function_compiler: it is an internal representation authority, not
// a backend-wide API.

/// Phase 1d typed-IR: recover a raw i64 operand from a variable that holds
/// raw i64 in its main Cranelift Variable.  The static `representation_plan`
/// set (Step 0's operand-recursive fixpoint) is the single source of truth:
/// `name ∈ representation_plan ⇒ use_var(vars[name])` yields a raw i64.
///
/// Cranelift's FunctionBuilder caches `use_var` within a block AND inserts
/// phi nodes automatically at block boundaries when a Variable has multiple
/// defs, so a single static-set lookup replaces the legacy two-tier shadow
/// plumbing (in-block SSA shadow values, cross-block shadow Variables, and
/// dynamic raw-carrier membership).
#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn int_raw_value(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    name: &str,
) -> Option<Value> {
    if representation_plan.is_raw_int_carrier_name(name)
        && let Some(&var) = vars.get(name)
    {
        return Some(builder.use_var(var));
    }
    None
}

/// Define a known-inline integer result under the Phase 1d representation
/// invariant:
///
/// - `out ∈ representation_plan` stores raw i64 in the main Variable.
/// - every other output stores a NaN-boxed Python int in the main Variable.
#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn def_inline_int_value(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    out: &str,
    raw_value: Value,
    boxed_value: i64,
) {
    if representation_plan.is_raw_int_carrier_name(out) {
        def_var_named(builder, vars, out, raw_value);
    } else {
        let boxed = builder.ins().iconst(types::I64, boxed_value);
        def_var_named(builder, vars, out, boxed);
    }
}

#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn emit_exception_pending_condition(
    builder: &mut FunctionBuilder<'_>,
    pointer_type: types::Type,
    local_exc_pending: FuncRef,
    exc_flag_ptr_slot: Option<cranelift_codegen::ir::StackSlot>,
) -> Value {
    let Some(slot) = exc_flag_ptr_slot else {
        let call = builder.ins().call(local_exc_pending, &[]);
        let pending = builder.inst_results(call)[0];
        return builder.ins().icmp_imm_s(IntCC::NotEqual, pending, 0);
    };
    // The record's byte is a hint (invariant J1): clear means nothing is
    // pending; set may be stale after a task switch, so the runtime reader
    // confirms it and repairs the byte, exactly as LLVM and WASM do.
    let record = builder.ins().stack_load(pointer_type, types::I64, slot, 0);
    let hint = builder.ins().load(
        types::I8,
        MemFlagsData::trusted(),
        record,
        molt_codegen_abi::THREAD_RECORD_EXCEPTION_PENDING_OFFSET,
    );
    let hint_set = builder.ins().icmp_imm_s(IntCC::NotEqual, hint, 0);
    let confirm = builder.create_block();
    let join = builder.create_block();
    let answer = builder.append_block_param(join, types::I8);
    builder.set_cold_block(confirm);
    // Both blocks are private to this check and complete once entered.
    brif_block(builder, hint_set, confirm, &[], join, &[hint_set]);
    switch_to_block_materialized(builder, confirm);
    builder.seal_block(confirm);
    let call = builder.ins().call(local_exc_pending, &[]);
    let pending = builder.inst_results(call)[0];
    let confirmed = builder.ins().icmp_imm_s(IntCC::NotEqual, pending, 0);
    jump_block(builder, join, &[confirmed]);
    switch_to_block_materialized(builder, join);
    builder.seal_block(join);
    answer
}

/// Store `location` into the frame's location word, at the canonical ABI
/// offset before the lent home base. The one native publication primitive.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn emit_frame_location_store(
    builder: &mut FunctionBuilder<'_>,
    address: Value,
    location: i64,
) {
    let location = builder.ins().iconst(types::I64, location);
    builder.ins().store(
        MemFlagsData::trusted(),
        location,
        address,
        molt_codegen_abi::FRAME_LOCATION_OFFSET,
    );
}

/// Read the thread's pending-work word and test it: true when work waits for
/// a poll.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn emit_async_work_pending(
    builder: &mut FunctionBuilder<'_>,
    work_ptr_slot: cranelift_codegen::ir::StackSlot,
) -> Value {
    let ptr = builder
        .ins()
        .stack_load(types::I64, types::I64, work_ptr_slot, 0);
    // Cranelift's atomic_load is SC (its only atomic-load ordering). A normal
    // load is unsound here: GVN/LICM may reuse it across an external request.
    let word = builder
        .ins()
        .atomic_load(types::I32, MemFlagsData::trusted(), ptr);
    builder.ins().icmp_imm_s(IntCC::NotEqual, word, 0)
}

/// Service async work only when its runtime word is set. The fast result is
/// still an exception observation: poll placement marks existing checks after
/// raising operations; it does not make their exception predicate redundant.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn emit_async_work_poll(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    work_ptr_slot: cranelift_codegen::ir::StackSlot,
    record_slot: cranelift_codegen::ir::StackSlot,
    cold_observer: FuncRef,
    // `molt_exception_pending`: confirms a set pending byte on the fast path,
    // which services no work.
    exception_pending: FuncRef,
    fast_observer: Option<FuncRef>,
    cold_location: Option<(cranelift_codegen::ir::StackSlot, i64)>,
) -> Value {
    let pending = emit_async_work_pending(builder, work_ptr_slot);
    let cold = builder.create_block();
    builder.set_cold_block(cold);
    let fast = builder.create_block();
    let done = builder.create_block();
    builder.append_block_param(done, types::I64);
    builder.ins().brif(pending, cold, &[], fast, &[]);
    switch_to_block_materialized(builder, cold);
    seal_block_once(builder, sealed_blocks, cold);
    if let Some((cold_homes, location)) = cold_location {
        // Serviced work is Python code that can read this frame.
        let homes = builder
            .ins()
            .stack_load(types::I64, types::I64, cold_homes, 0);
        emit_frame_location_store(builder, homes, location);
    }
    let call = builder.ins().call(cold_observer, &[]);
    let result = builder.inst_results(call)[0];
    jump_block(builder, done, &[result]);
    switch_to_block_materialized(builder, fast);
    seal_block_once(builder, sealed_blocks, fast);
    let cond =
        emit_exception_pending_condition(builder, types::I64, exception_pending, Some(record_slot));
    let result = if let Some(observer) = fast_observer {
        let capture = builder.create_block();
        let none = builder
            .ins()
            .iconst(types::I64, molt_codegen_abi::QNAN_TAG_NONE_I64);
        brif_block(builder, cond, capture, &[], done, &[none]);
        switch_to_block_materialized(builder, capture);
        seal_block_once(builder, sealed_blocks, capture);
        let call = builder.ins().call(observer, &[]);
        builder.inst_results(call)[0]
    } else {
        builder.ins().uextend(types::I64, cond)
    };
    jump_block(builder, done, &[result]);
    switch_to_block_materialized(builder, done);
    seal_block_once(builder, sealed_blocks, done);
    builder.block_params(done)[0]
}

#[cfg(feature = "native-backend")]
/// Preserve the boxed runtime result unless semantic representation facts prove
/// an exact Boolean. Rich comparisons may return arbitrary Python objects.
pub(in crate::native_backend::function_compiler) fn def_bool_result(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    out: &str,
    boxed_result: Value,
    raw_bool: Option<Value>,
) {
    if representation_plan.is_bool_unboxed(out) {
        let raw_bool = raw_bool.unwrap_or_else(|| builder.ins().band_imm_u(boxed_result, 1));
        def_var_named(builder, vars, out, raw_bool);
    } else {
        def_var_named(builder, vars, out, boxed_result);
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn def_raw_bool_value(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    out: &str,
    raw_bool: Value,
    nbc: &crate::NanBoxConsts,
) {
    if representation_plan.is_bool_unboxed(out) {
        def_var_named(builder, vars, out, raw_bool);
        return;
    }
    let boxed = box_raw_bool_value(builder, raw_bool, nbc);
    def_bool_result(
        builder,
        vars,
        representation_plan,
        out,
        boxed,
        Some(raw_bool),
    );
}

/// Read a raw 0/1 bool only from a bool-primary Variable.
#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn bool_raw_value(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    name: &str,
) -> Option<Value> {
    if representation_plan.is_bool_unboxed(name)
        && let Some(&var) = vars.get(name)
    {
        return Some(builder.use_var(var));
    }
    None
}

/// Produce a correctly NaN-boxed value for a variable in `representation_plan`
/// using its canonical range proof. Proven inline integers use mask/or only.
///
/// For full-width carriers, a deferred overflow guard is emitted at boxing
/// escape points (return, call args, heap stores) to compensate for the branchless iadd optimisation
/// that skips per-op overflow checks. On the fast path (fits inline) emits
/// a compare + branch into local-constant `box_int_value`; on the cold path calls
/// `molt_int_from_i64` to allocate a BigInt.
///
/// Falls back to `var_get` (the normal boxed path) when the variable is
/// not raw-primary.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn ensure_boxed_overflow_safe(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    name: &str,
) -> Value {
    let raw_val = int_raw_value(builder, vars, representation_plan, name);
    if let Some(raw_val) = raw_val {
        let boxed = if representation_plan.is_inline_safe_int_name(name) {
            box_int_value(builder, raw_val, &crate::NanBoxConsts::new())
        } else {
            box_raw_i64_value_overflow_safe(
                module,
                import_ids,
                builder,
                import_refs,
                sealed_blocks,
                raw_val,
            )
        };
        if let Some(&var) = vars.get(name) {
            builder.def_var(var, raw_val);
        }
        boxed
    } else {
        *var_get(builder, vars, name).expect("Variable not found for overflow-safe boxing")
    }
}

/// Box a raw i64 VALUE overflow-safely: fits-inline fast path (band+bor NaN
/// box) with a cold `molt_int_from_i64` BigInt slow path. The value-level
/// core of [`ensure_boxed_overflow_safe`], shared by the raw-backed join-slot
/// load path (which has a `Value` from a `stack_load`, not a named Variable).
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn box_raw_i64_value_overflow_safe(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    sealed_blocks: &mut BTreeSet<Block>,
    raw_val: Value,
) -> Value {
    let fits = int_value_fits_inline(builder, raw_val);
    let fast_blk = builder.create_block();
    let slow_blk = builder.create_block();
    builder.set_cold_block(slow_blk);
    let merge_blk = builder.create_block();
    builder.append_block_param(merge_blk, types::I64);
    builder.ins().brif(fits, fast_blk, &[], slow_blk, &[]);

    switch_to_block_materialized(builder, fast_blk);
    seal_block_once(builder, sealed_blocks, fast_blk);
    // Escape boxing is representation-critical: do not read the NaN-box mask/tag
    // from Cranelift Variables here. Split-heavy CFGs can repair those Variables
    // through implicit block params; local constants make RawI64 -> BoxedI64 an
    // explicit boundary independent of SSA variable repair.
    let nbc = crate::NanBoxConsts::new();
    let int_mask = builder.ins().iconst(types::I64, nbc.int_mask);
    let masked = builder.ins().band(raw_val, int_mask);
    let int_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    let inline_boxed = builder.ins().bor(int_tag, masked);
    jump_block(builder, merge_blk, &[inline_boxed]);

    switch_to_block_materialized(builder, slow_blk);
    seal_block_once(builder, sealed_blocks, slow_blk);
    let big_fn = import_runtime_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        RuntimeFn::IntFromI64,
    );
    let big_call = builder.ins().call(big_fn, &[raw_val]);
    let big_res = builder.inst_results(big_call)[0];
    jump_block(builder, merge_blk, &[big_res]);

    switch_to_block_materialized(builder, merge_blk);
    seal_block_once(builder, sealed_blocks, merge_blk);
    builder.block_params(merge_blk)[0]
}

/// Phase 1d typed-IR: overflow-safe sibling of `var_get_boxed` for boxing
/// escape points (function return, call arguments, heap stores, runtime
/// print/repr/str, exception rebind, def_var_global).
///
/// Internally dispatches int-primary names to `ensure_boxed_overflow_safe`
/// (which has a fits-inline fast path + a cold `molt_int_from_i64` BigInt
/// slow path), float-primary names through the backend's NaN-canonicalizing
/// raw-F64 -> boxed-I64 boundary, and otherwise returns the already-NaN-boxed
/// main Variable verbatim.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn var_get_boxed_overflow_safe_base(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    name: &str,
    representation_plan: &ScalarRepresentationPlan,
) -> Option<crate::VarValue> {
    use crate::VarValue;
    if representation_plan.is_raw_int_carrier_name(name) {
        let boxed = ensure_boxed_overflow_safe(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            representation_plan,
            name,
        );
        Some(VarValue(boxed))
    } else if representation_plan.is_float_unboxed(name) {
        let var = *vars.get(name)?;
        let val = builder.use_var(var);
        let bits = box_float_value(builder, val);
        Some(VarValue(bits))
    } else {
        var_get(builder, vars, name)
    }
}

/// Box a known-bool variable's raw 0/1 value into a TAG_BOOL NaN-box.
///
/// Bool-primary variables carry raw 0/1 in their main Cranelift Variable.
/// Non-primary bool-typed variables carry boxed TAG_BOOL values.
#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn box_raw_bool_value(
    builder: &mut FunctionBuilder<'_>,
    raw_bool: Value,
    nbc: &crate::NanBoxConsts,
) -> Value {
    let cond = builder.ins().icmp_imm_s(IntCC::NotEqual, raw_bool, 0);
    box_bool_value(builder, cond, nbc)
}

/// Return the value that should flow through the existing boxed-result compare
/// lowering path plus the raw 0/1 carrier for bool-primary consumers.
///
/// When the destination is bool-primary, the boxed value would be immediately
/// discarded by `def_bool_result`; do not emit it. Non-primary bool results keep
/// the NaN-boxed representation required by generic consumers.
#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn compare_bool_result_value(
    builder: &mut FunctionBuilder<'_>,
    representation_plan: &ScalarRepresentationPlan,
    out: Option<&String>,
    cond: Value,
    nbc: &crate::NanBoxConsts,
) -> (Value, Value) {
    let raw_bool = builder.ins().uextend(types::I64, cond);
    let result = if out.is_some_and(|out| representation_plan.is_bool_unboxed(out)) {
        raw_bool
    } else {
        box_bool_value(builder, cond, nbc)
    };
    (result, raw_bool)
}

/// Truthiness carrier for an unknown-list getitem whose source list has a
/// snapshotted runtime `TYPE_ID_LIST_BOOL` check.
///
/// `payload` is raw 0/1 only when `is_bool` was true at the element read; when
/// the list is a regular list, the same payload is the NaN-boxed element and
/// consumers must continue through the normal tag/runtime truthiness path.
#[cfg(feature = "native-backend")]
#[derive(Clone)]
pub(in crate::native_backend::function_compiler) struct ConditionalListBoolShadow {
    pub(in crate::native_backend::function_compiler) is_bool: Value,
    pub(in crate::native_backend::function_compiler) payload: Value,
}

#[cfg(feature = "native-backend")]
impl ConditionalListBoolShadow {
    pub(in crate::native_backend::function_compiler) fn from_boxed(
        builder: &mut FunctionBuilder<'_>,
        is_bool: Value,
        boxed: Value,
    ) -> Self {
        // Runtime fallback (notably negative indexing) returns a boxed value
        // even for list_bool. Match the fast path's layout-dependent payload.
        let raw_bit = builder.ins().band_imm_u(boxed, 1);
        let payload = builder.ins().select(is_bool, raw_bit, boxed);
        Self { is_bool, payload }
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn emit_conditional_list_bool_truthiness(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    shadow: Option<&ConditionalListBoolShadow>,
    truthy_merge: Block,
    live_through: &[LiveThroughValue],
) -> bool {
    let Some(shadow) = shadow else {
        return false;
    };
    let zero_i8 = builder.ins().iconst(types::I8, 0);
    let is_bool_check = builder.ins().icmp(IntCC::NotEqual, shadow.is_bool, zero_i8);
    let raw_bool_block = builder.create_block();
    let speculative_block = builder.create_block();
    builder
        .ins()
        .brif(is_bool_check, raw_bool_block, &[], speculative_block, &[]);

    switch_to_block_materialized(builder, raw_bool_block);
    seal_block_once(builder, sealed_blocks, raw_bool_block);
    let raw_truthy = builder.ins().icmp_imm_s(IntCC::NotEqual, shadow.payload, 0);
    let merge_args = merge_args_with_live_through(raw_truthy, live_through);
    jump_block(builder, truthy_merge, &merge_args);

    switch_to_block_materialized(builder, speculative_block);
    seal_block_once(builder, sealed_blocks, speculative_block);
    true
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn ensure_boxed_bool_safe(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    name: &str,
) -> Option<Value> {
    let raw_val = bool_raw_value(builder, vars, representation_plan, name)
        .or_else(|| int_raw_value(builder, vars, representation_plan, name));
    if let Some(raw_val) = raw_val {
        return Some(box_raw_bool_value(builder, raw_val, nbc));
    }
    var_get(builder, vars, name).map(|v| v.0)
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn ensure_boxed_primitive_safe(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    nbc: &crate::NanBoxConsts,
    representation_plan: &ScalarRepresentationPlan,
    name: &str,
) -> Value {
    if representation_plan.is_float_unboxed(name) {
        *var_get_boxed_overflow_safe_base(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
        )
        .expect("float escape var not found")
    } else if representation_plan.name_is_bool_scalar(name) {
        ensure_boxed_bool_safe(builder, vars, representation_plan, nbc, name)
            .expect("bool variable not found for primitive-safe boxing")
    } else {
        ensure_boxed_overflow_safe(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            representation_plan,
            name,
        )
    }
}

/// Define a named backend variable from boxed I64 transport.
///
/// Stack slots, closure state, future/channel payloads, runtime call returns,
/// and return edges use boxed I64 transport unless they have an explicit typed
/// ABI. Native scalar-primary variables, however, have raw homes (`I64` for
/// int/bool and `F64` for float). This helper is the authoritative boundary
/// between those contracts: callers that hold boxed transport must use it
/// instead of writing directly through `def_var_named`.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn def_var_from_boxed_transport(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    name: &str,
    boxed: Value,
) {
    if representation_plan.is_float_unboxed(name) {
        let raw_f64 =
            float_value_from_boxed_extended(module, import_ids, builder, import_refs, boxed);
        def_var_named(builder, vars, name, raw_f64);
    } else if representation_plan.is_bool_unboxed(name) {
        let raw_bool = builder.ins().band_imm_u(boxed, 1);
        def_var_named(builder, vars, name, raw_bool);
    } else if representation_plan.is_inline_safe_int_name(name) {
        let raw_i64 = unbox_int_or_bool(builder, boxed, nbc);
        def_var_named(builder, vars, name, raw_i64);
    } else if representation_plan.is_full_deopt_int_name(name) {
        panic!(
            "boxed transport cannot define full-deopt raw-int home `{name}`; \
             use a raw i64 producer or checked raw runtime ABI"
        );
    } else {
        def_var_named(builder, vars, name, boxed);
    }
}

/// Define a named backend variable from the result of a numeric lowering that
/// may choose either a raw-F64 lane or boxed-I64 runtime transport.
///
/// Integer raw-primary paths return early before reaching this helper. When a
/// surviving result is `I64`, it is boxed transport and must cross the same
/// representation boundary as runtime call returns before entering scalar
/// primary homes.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn def_var_from_numeric_result(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    name: &str,
    value: Value,
) {
    match builder.func.dfg.value_type(value) {
        types::F64 => def_var_named(builder, vars, name, value),
        types::I64 => def_var_from_boxed_transport(
            module,
            import_ids,
            builder,
            import_refs,
            vars,
            representation_plan,
            nbc,
            name,
            value,
        ),
        ty => panic!("numeric result for {name} has unsupported CLIF type {ty}"),
    }
}

#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn merge_rebind_storage_for_name(
    name: &str,
    representation_plan: &ScalarRepresentationPlan,
) -> MergeRebindStorageKind {
    if representation_plan.is_float_unboxed(name) {
        MergeRebindStorageKind::RawF64
    } else if representation_plan.is_bool_unboxed(name) {
        MergeRebindStorageKind::RawBool
    } else if representation_plan.is_raw_int_carrier_name(name) {
        MergeRebindStorageKind::RawI64
    } else {
        MergeRebindStorageKind::BoxedI64
    }
}

#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn merge_rebind_storage_clif_type(
    storage: MergeRebindStorageKind,
) -> types::Type {
    match storage {
        MergeRebindStorageKind::RawF64 => types::F64,
        MergeRebindStorageKind::BoxedI64
        | MergeRebindStorageKind::RawI64
        | MergeRebindStorageKind::RawBool => types::I64,
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn merge_rebind_default_value(
    builder: &mut FunctionBuilder<'_>,
    storage: MergeRebindStorageKind,
) -> Value {
    match storage {
        MergeRebindStorageKind::RawF64 => builder.ins().f64const(0.0),
        MergeRebindStorageKind::BoxedI64
        | MergeRebindStorageKind::RawI64
        | MergeRebindStorageKind::RawBool => builder.ins().iconst(types::I64, 0),
    }
}

/// A captured incoming value, independent of subsequent writes to its name.
/// All consumers of one transfer share its lazy boxed materialization. Raw
/// transfers never allocate; an escaping raw integer supplies one owned box.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) struct CapturedScalarTransport {
    value: Value,
    storage: MergeRebindStorageKind,
    // This proof belongs to the captured value, not a subsequently rebound name.
    inline_safe_int: bool,
    boxed: Option<Value>,
    boxed_owner_available: bool,
}

#[cfg(feature = "native-backend")]
impl CapturedScalarTransport {
    pub(super) fn new(value: Value, storage: MergeRebindStorageKind) -> Self {
        Self {
            value,
            storage,
            inline_safe_int: false,
            boxed: (storage == MergeRebindStorageKind::BoxedI64).then_some(value),
            boxed_owner_available: storage != MergeRebindStorageKind::BoxedI64,
        }
    }

    pub(super) fn read_named(
        builder: &mut FunctionBuilder<'_>,
        vars: &BTreeMap<String, Variable>,
        representation_plan: &ScalarRepresentationPlan,
        name: &str,
    ) -> Self {
        let storage = merge_rebind_storage_for_name(name, representation_plan);
        let value = *var_get(builder, vars, name).expect("scalar transport source not found");
        Self {
            inline_safe_int: representation_plan.is_inline_safe_int_name(name),
            ..Self::new(value, storage)
        }
    }

    pub(super) fn read_local(
        builder: &mut FunctionBuilder<'_>,
        pointer_type: types::Type,
        vars: &BTreeMap<String, Variable>,
        representation_plan: &ScalarRepresentationPlan,
        slots: &BTreeMap<String, cranelift_codegen::ir::StackSlot>,
        raw_slots: &BTreeSet<String>,
        name: &str,
    ) -> Self {
        if let Some(&slot) = slots.get(name) {
            let storage = if raw_slots.contains(name) {
                merge_rebind_storage_for_name(name, representation_plan)
            } else {
                MergeRebindStorageKind::BoxedI64
            };
            let value = builder.ins().stack_load(
                pointer_type,
                merge_rebind_storage_clif_type(storage),
                slot,
                0,
            );
            Self {
                inline_safe_int: storage == MergeRebindStorageKind::RawI64
                    && representation_plan.is_inline_safe_int_name(name),
                ..Self::new(value, storage)
            }
        } else {
            Self::read_named(builder, vars, representation_plan, name)
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn value_for_home(
        &mut self,
        module: &mut dyn Module,
        import_ids: &mut RuntimeImportIds,
        builder: &mut FunctionBuilder<'_>,
        import_refs: &mut NativeFunctionImports,
        sealed_blocks: &mut BTreeSet<Block>,
        representation_plan: &ScalarRepresentationPlan,
        nbc: &crate::NanBoxConsts,
        name: &str,
        storage: MergeRebindStorageKind,
    ) -> Value {
        assert!(
            storage != MergeRebindStorageKind::RawI64
                || !representation_plan.is_full_deopt_int_name(name)
                || matches!(
                    self.storage,
                    MergeRebindStorageKind::RawI64 | MergeRebindStorageKind::RawBool
                ),
            "boxed transport cannot define full-deopt raw-int home `{name}`; use a raw i64 producer or checked raw runtime ABI"
        );
        self.value_for_storage(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            nbc,
            storage,
        )
    }

    /// Consume the materialization's existing credit before retaining another
    /// independent boxed owner. Inline scalars make either operation a no-op.
    pub(super) fn take_boxed_owner(&mut self) -> bool {
        assert!(
            self.boxed.is_some(),
            "boxed ownership requires materialization"
        );
        std::mem::replace(&mut self.boxed_owner_available, false)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn value_for_storage(
        &mut self,
        module: &mut dyn Module,
        import_ids: &mut RuntimeImportIds,
        builder: &mut FunctionBuilder<'_>,
        import_refs: &mut NativeFunctionImports,
        sealed_blocks: &mut BTreeSet<Block>,
        nbc: &crate::NanBoxConsts,
        storage: MergeRebindStorageKind,
    ) -> Value {
        if self.storage == storage
            || matches!(
                (self.storage, storage),
                (
                    MergeRebindStorageKind::RawI64,
                    MergeRebindStorageKind::RawBool
                ) | (
                    MergeRebindStorageKind::RawBool,
                    MergeRebindStorageKind::RawI64
                )
            )
        {
            return self.value;
        }
        if storage == MergeRebindStorageKind::RawF64
            && matches!(
                self.storage,
                MergeRebindStorageKind::RawI64 | MergeRebindStorageKind::RawBool
            )
        {
            return builder.ins().fcvt_from_sint(types::F64, self.value);
        }
        let boxed = if let Some(boxed) = self.boxed {
            boxed
        } else {
            let boxed = match self.storage {
                MergeRebindStorageKind::RawI64 if self.inline_safe_int => {
                    box_int_value(builder, self.value, nbc)
                }
                MergeRebindStorageKind::RawI64 => box_raw_i64_value_overflow_safe(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    sealed_blocks,
                    self.value,
                ),
                MergeRebindStorageKind::RawF64 => box_float_value(builder, self.value),
                MergeRebindStorageKind::RawBool => box_raw_bool_value(builder, self.value, nbc),
                MergeRebindStorageKind::BoxedI64 => unreachable!(),
            };
            self.boxed = Some(boxed);
            boxed
        };
        match storage {
            MergeRebindStorageKind::BoxedI64 => boxed,
            MergeRebindStorageKind::RawF64 => {
                float_value_from_boxed_extended(module, import_ids, builder, import_refs, boxed)
            }
            MergeRebindStorageKind::RawI64 => unbox_int_or_bool(builder, boxed, nbc),
            MergeRebindStorageKind::RawBool => builder.ins().band_imm_u(boxed, 1),
        }
    }
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn merge_rebind_value_for_storage(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    name: &str,
    storage: MergeRebindStorageKind,
) -> Value {
    CapturedScalarTransport::read_named(builder, vars, representation_plan, name).value_for_storage(
        module,
        import_ids,
        builder,
        import_refs,
        sealed_blocks,
        nbc,
        storage,
    )
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn def_var_from_merge_rebind_storage(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    name: &str,
    value: Value,
    storage: MergeRebindStorageKind,
) {
    match storage {
        MergeRebindStorageKind::RawF64 | MergeRebindStorageKind::RawI64 => {
            def_var_named(builder, vars, name, value);
        }
        MergeRebindStorageKind::RawBool => {
            def_raw_bool_value(builder, vars, representation_plan, name, value, nbc);
        }
        MergeRebindStorageKind::BoxedI64 => {
            def_var_from_boxed_transport(
                module,
                import_ids,
                builder,
                import_refs,
                vars,
                representation_plan,
                nbc,
                name,
                value,
            );
        }
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) struct LiveThroughValue {
    name: String,
    value: Value,
    ty: cranelift_codegen::ir::Type,
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn collect_live_through_values(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    first_defined_at: &BTreeMap<String, usize>,
    last_use: &BTreeMap<String, usize>,
    op_idx: usize,
    current_out: Option<&str>,
) -> Vec<LiveThroughValue> {
    first_defined_at
        .iter()
        .filter_map(|(name, first)| {
            if name == "none" || current_out == Some(name.as_str()) {
                return None;
            }
            if name.ends_with("_ptr") || name.ends_with("_len") {
                return None;
            }
            if *first >= op_idx || last_use.get(name).copied().unwrap_or(0) <= op_idx {
                return None;
            }
            let var = *vars.get(name)?;
            let value = builder.use_var(var);
            let ty = builder.func.dfg.value_type(value);
            Some(LiveThroughValue {
                name: name.clone(),
                value,
                ty,
            })
        })
        .collect()
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn append_live_through_params(
    builder: &mut FunctionBuilder<'_>,
    block: Block,
    live_through: &[LiveThroughValue],
) {
    for live in live_through {
        builder.append_block_param(block, live.ty);
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn merge_args_with_live_through(
    head: Value,
    live_through: &[LiveThroughValue],
) -> Vec<Value> {
    let mut args = Vec::with_capacity(1 + live_through.len());
    args.push(head);
    args.extend(live_through.iter().map(|live| live.value));
    args
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn rebind_live_through_values(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    live_through: &[LiveThroughValue],
    params: &[Value],
) {
    for (live, param) in live_through.iter().zip(params.iter().copied()) {
        def_var_named(builder, vars, live.name.clone(), param);
    }
}

#[cfg(feature = "native-backend")]
#[derive(Clone, Copy)]
pub(in crate::native_backend::function_compiler) enum BoxedBitwiseOp {
    And,
    Or,
    Xor,
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn emit_guarded_boxed_bitwise(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    sealed_blocks: &mut BTreeSet<Block>,
    lhs: Value,
    rhs: Value,
    runtime_name: RuntimeFn,
    op: BoxedBitwiseOp,
    nbc: &crate::NanBoxConsts,
) -> Value {
    let callee = import_runtime_func_ref(module, import_ids, builder, import_refs, runtime_name);
    let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(builder, lhs, nbc);
    let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(builder, rhs, nbc);
    let both_inline_int = fused_both_int_check(builder, lhs_xored, rhs_xored, nbc);
    let fast_block = builder.create_block();
    let slow_block = builder.create_block();
    builder.set_cold_block(slow_block);
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, types::I64);
    builder
        .ins()
        .brif(both_inline_int, fast_block, &[], slow_block, &[]);

    switch_to_block_materialized(builder, fast_block);
    seal_block_once(builder, sealed_blocks, fast_block);
    let raw = match op {
        BoxedBitwiseOp::And => builder.ins().band(lhs_val, rhs_val),
        BoxedBitwiseOp::Or => builder.ins().bor(lhs_val, rhs_val),
        BoxedBitwiseOp::Xor => builder.ins().bxor(lhs_val, rhs_val),
    };
    let fast_res = box_int_value(builder, raw, nbc);
    let fits_inline = int_value_fits_inline(builder, raw);
    brif_block(
        builder,
        fits_inline,
        merge_block,
        &[fast_res],
        slow_block,
        &[],
    );

    switch_to_block_materialized(builder, slow_block);
    seal_block_once(builder, sealed_blocks, slow_block);
    let call = builder.ins().call(callee, &[lhs, rhs]);
    let slow_res = builder.inst_results(call)[0];
    jump_block(builder, merge_block, &[slow_res]);

    switch_to_block_materialized(builder, merge_block);
    seal_block_once(builder, sealed_blocks, merge_block);
    builder.block_params(merge_block)[0]
}

/// Get a raw f64 value from a variable, checking float-primary Variables
/// only. Non-primary float values live in their main boxed I64 Variable and
/// are recovered through the runtime's extended float extractor at use sites.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn float_value_for(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    name: &str,
) -> Option<Value> {
    if representation_plan.is_float_unboxed(name) {
        return vars.get(name).map(|&var| builder.use_var(var));
    }
    None
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn dead_scrub_value_for_var(
    builder: &mut FunctionBuilder<'_>,
    representation_plan: &ScalarRepresentationPlan,
    name: &str,
) -> Value {
    if representation_plan.is_float_unboxed(name) {
        builder.ins().f64const(0.0)
    } else {
        builder.ins().iconst(types::I64, 0)
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn float_value_from_boxed_extended(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    boxed: Value,
) -> Value {
    let as_float = import_runtime_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        RuntimeFn::FloatAsF64,
    );
    let call = builder.ins().call(as_float, &[boxed]);
    builder.inst_results(call)[0]
}

/// Read the numeric payload authorized by a bound producer witness. Raw integer
/// carriers convert directly; exact boxed bools use their payload bit; boxed
/// floats use the extended extractor, including heap NaNs. Unknown-magnitude
/// boxed integers remain on generic runtime dispatch.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn float_value_for_mixed(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    operand: crate::representation_plan::NumericOperand<'_>,
) -> Value {
    let name = operand.name();
    // The operand carries a producer witness; an annotation cannot reach here.
    // 1. Try float-primary path first.
    if let Some(f_val) = float_value_for(builder, vars, representation_plan, name) {
        return f_val;
    }

    // 2. Operand is int — get raw i64 and convert to f64.
    if matches!(operand.kind(), ScalarKind::Int | ScalarKind::Bool) {
        // Phase 1d: representation_plan members hold raw i64 in the main
        // Variable; reading via int_raw_value avoids the box/unbox detour.
        if let Some(raw_int_val) = int_raw_value(builder, vars, representation_plan, name) {
            return builder.ins().fcvt_from_sint(types::F64, raw_int_val);
        }
        // An exact boxed Boolean has only payload bit zero. Integer
        // operands without a raw carrier are rejected by NumericOperand.
        let boxed = var_get_boxed_overflow_safe_base(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
        )
        .expect("Numeric operand not found");
        assert_eq!(
            operand.kind(),
            ScalarKind::Bool,
            "integer producer lacks its raw carrier"
        );
        let raw = builder.ins().band_imm_u(*boxed, 1);
        return builder.ins().fcvt_from_sint(types::F64, raw);
    }

    // 3. Not int — must be a boxed float. Use the runtime's extended float
    // extractor so heap-allocated NaN floats recover their IEEE payload rather
    // than being reinterpreted as pointer-tagged bits.
    let boxed = var_get_boxed_overflow_safe_base(
        module,
        import_ids,
        builder,
        import_refs,
        sealed_blocks,
        vars,
        name,
        representation_plan,
    )
    .expect("Float operand not found");
    float_value_from_boxed_extended(module, import_ids, builder, import_refs, *boxed)
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn emit_float_numeric_compare(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    out_name: Option<&String>,
    operands: crate::representation_plan::FloatComparisonOperands<'_>,
    cc: FloatCC,
) -> (Value, Value) {
    let (lhs, rhs) = operands.operands();
    let lhs_f = float_value_for_mixed(
        module,
        import_ids,
        builder,
        import_refs,
        sealed_blocks,
        vars,
        representation_plan,
        lhs,
    );
    let rhs_f = float_value_for_mixed(
        module,
        import_ids,
        builder,
        import_refs,
        sealed_blocks,
        vars,
        representation_plan,
        rhs,
    );
    let cmp = builder.ins().fcmp(cc, lhs_f, rhs_f);
    compare_bool_result_value(builder, representation_plan, out_name, cmp, nbc)
}
