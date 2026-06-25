use super::*;

// Scalar carrier, boxing, and transport-boundary helpers shared by the native
// function compiler shell and extracted fc::* op families. Keep this module
// private to function_compiler: it is an internal representation authority, not
// a backend-wide API.

#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn name_is_int_like(
    name: &str,
    int_like_vars: &BTreeSet<String>,
    bool_like_vars: &BTreeSet<String>,
) -> bool {
    int_like_vars.contains(name) || bool_like_vars.contains(name)
}

/// Phase 1d typed-IR: recover a raw i64 operand from a variable that holds
/// raw i64 in its main Cranelift Variable.  The static `int_primary_vars`
/// set (Step 0's operand-recursive fixpoint) is the single source of truth:
/// `name ∈ int_primary_vars ⇒ use_var(vars[name])` yields a raw i64.
///
/// Cranelift's FunctionBuilder caches `use_var` within a block AND inserts
/// phi nodes automatically at block boundaries when a Variable has multiple
/// defs, so a single static-set lookup replaces the legacy two-tier shadow
/// plumbing (`raw_int_shadow_vals` for in-block SSA values, `raw_int_shadow`
/// Variable-tier for cross-block phi, dynamic `raw_primary_int` for
/// membership).
#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn int_raw_value(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    int_primary_vars: &BTreeSet<String>,
    name: &str,
) -> Option<Value> {
    if int_primary_vars.contains(name)
        && let Some(&var) = vars.get(name)
    {
        return Some(builder.use_var(var));
    }
    None
}

/// Define a known-inline integer result under the Phase 1d representation
/// invariant:
///
/// - `out ∈ int_primary_vars` stores raw i64 in the main Variable.
/// - every other output stores a NaN-boxed Python int in the main Variable.
#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn def_inline_int_value(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    int_primary_vars: &BTreeSet<String>,
    out: &str,
    raw_value: Value,
    boxed_value: i64,
) {
    if int_primary_vars.contains(out) {
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
    local_exc_pending_fast: FuncRef,
    exc_flag_ptr_slot: Option<cranelift_codegen::ir::StackSlot>,
) -> Value {
    if let Some(slot) = exc_flag_ptr_slot {
        let flag_ptr = builder.ins().stack_load(types::I64, slot, 0);
        let pending_byte = builder
            .ins()
            .load(types::I8, MemFlagsData::trusted(), flag_ptr, 0);
        return builder.ins().icmp_imm(IntCC::NotEqual, pending_byte, 0);
    }

    let call = builder.ins().call(local_exc_pending_fast, &[]);
    let pending = builder.inst_results(call)[0];
    builder.ins().icmp_imm(IntCC::NotEqual, pending, 0)
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn def_bool_result(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    bool_primary_vars: &BTreeSet<String>,
    out: &str,
    boxed_bool: Value,
    raw_bool: Option<Value>,
) {
    let raw_bool = raw_bool.unwrap_or_else(|| builder.ins().band_imm(boxed_bool, 1));
    if bool_primary_vars.contains(out) {
        def_var_named(builder, vars, out, raw_bool);
    } else {
        def_var_named(builder, vars, out, boxed_bool);
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn def_raw_bool_value(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    bool_primary_vars: &BTreeSet<String>,
    out: &str,
    raw_bool: Value,
    nbc: &crate::NanBoxConsts,
) {
    if bool_primary_vars.contains(out) {
        def_var_named(builder, vars, out, raw_bool);
        return;
    }
    let boxed = box_raw_bool_value(builder, raw_bool, nbc);
    def_bool_result(builder, vars, bool_primary_vars, out, boxed, Some(raw_bool));
}

/// Read a raw 0/1 bool only from a bool-primary Variable.
#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn bool_raw_value(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    bool_primary_vars: &BTreeSet<String>,
    name: &str,
) -> Option<Value> {
    if bool_primary_vars.contains(name)
        && let Some(&var) = vars.get(name)
    {
        return Some(builder.use_var(var));
    }
    None
}

/// Produce a correctly NaN-boxed value for a variable in `int_primary_vars`
/// whose raw i64 may exceed the 47-bit inline range.
///
/// Deferred overflow guard emitted at boxing escape points (return, call
/// args, heap stores) to compensate for the branchless iadd optimisation
/// that skips per-op overflow checks. On the fast path (fits inline) emits
/// a compare + branch into local-constant `box_int_value`; on the cold path calls
/// `molt_int_from_i64` to allocate a BigInt.
///
/// Falls back to `var_get` (the normal boxed path) when the variable is
/// not raw-primary.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn ensure_boxed_overflow_safe(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    int_primary_vars: &std::collections::BTreeSet<String>,
    name: &str,
) -> Value {
    let raw_val = int_raw_value(builder, vars, int_primary_vars, name);
    if let Some(raw_val) = raw_val {
        let boxed = box_raw_i64_value_overflow_safe(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            raw_val,
        );
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
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
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
    let nbc = crate::NanBoxConsts::new(builder);
    let int_mask = builder.ins().iconst(types::I64, nbc.int_mask);
    let masked = builder.ins().band(raw_val, int_mask);
    let int_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
    let inline_boxed = builder.ins().bor(int_tag, masked);
    jump_block(builder, merge_blk, &[inline_boxed]);

    switch_to_block_materialized(builder, slow_blk);
    seal_block_once(builder, sealed_blocks, slow_blk);
    let big_fn = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        "molt_int_from_i64",
        &[types::I64],
        &[types::I64],
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
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    name: &str,
    int_primary_vars: &std::collections::BTreeSet<String>,
    float_primary_vars: &std::collections::BTreeSet<String>,
) -> Option<crate::VarValue> {
    use crate::VarValue;
    if int_primary_vars.contains(name) {
        let boxed = ensure_boxed_overflow_safe(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            int_primary_vars,
            name,
        );
        Some(VarValue(boxed))
    } else if float_primary_vars.contains(name) {
        let var = *vars.get(name)?;
        let val = builder.use_var(var);
        let nbc = crate::NanBoxConsts::new(builder);
        let bits = box_float_value(builder, val, &nbc);
        Some(VarValue(bits))
    } else {
        let var = *vars.get(name)?;
        let val = builder.use_var(var);
        Some(VarValue(val))
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
    let cond = builder.ins().icmp_imm(IntCC::NotEqual, raw_bool, 0);
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
    bool_primary_vars: &BTreeSet<String>,
    out: Option<&String>,
    cond: Value,
    nbc: &crate::NanBoxConsts,
) -> (Value, Value) {
    let raw_bool = builder.ins().uextend(types::I64, cond);
    let result = if out.is_some_and(|out| bool_primary_vars.contains(out)) {
        raw_bool
    } else {
        box_bool_value(builder, cond, nbc)
    };
    (result, raw_bool)
}

/// Truthiness carrier for an unknown-list getitem whose source list has a
/// cached runtime `TYPE_ID_LIST_BOOL` check.
///
/// `payload` is raw 0/1 only when `list_name` is a list_bool at runtime; when
/// the list is a regular list, the same payload is the NaN-boxed element and
/// consumers must continue through the normal tag/runtime truthiness path.
#[cfg(feature = "native-backend")]
#[derive(Clone)]
pub(in crate::native_backend::function_compiler) struct ConditionalListBoolShadow {
    pub(in crate::native_backend::function_compiler) list_name: String,
    pub(in crate::native_backend::function_compiler) payload: Value,
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn emit_conditional_list_bool_truthiness(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    list_is_bool_cache: &BTreeMap<String, Variable>,
    shadow: Option<&ConditionalListBoolShadow>,
    truthy_merge: Block,
    live_through: &[LiveThroughValue],
) -> bool {
    let Some(shadow) = shadow else {
        return false;
    };
    let Some(&ibvar) = list_is_bool_cache.get(&shadow.list_name) else {
        return false;
    };

    let ib = builder.use_var(ibvar);
    let zero_i8 = builder.ins().iconst(types::I8, 0);
    let is_bool_check = builder.ins().icmp(IntCC::NotEqual, ib, zero_i8);
    let raw_bool_block = builder.create_block();
    let speculative_block = builder.create_block();
    builder
        .ins()
        .brif(is_bool_check, raw_bool_block, &[], speculative_block, &[]);

    switch_to_block_materialized(builder, raw_bool_block);
    seal_block_once(builder, sealed_blocks, raw_bool_block);
    let raw_truthy = builder.ins().icmp_imm(IntCC::NotEqual, shadow.payload, 0);
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
    bool_primary_vars: &BTreeSet<String>,
    int_primary_vars: &BTreeSet<String>,
    nbc: &crate::NanBoxConsts,
    name: &str,
) -> Option<Value> {
    let raw_val = bool_raw_value(builder, vars, bool_primary_vars, name)
        .or_else(|| int_raw_value(builder, vars, int_primary_vars, name));
    if let Some(raw_val) = raw_val {
        return Some(box_raw_bool_value(builder, raw_val, nbc));
    }
    var_get(builder, vars, name).map(|v| v.0)
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn ensure_boxed_primitive_safe(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    bool_like_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    vars: &BTreeMap<String, Variable>,
    nbc: &crate::NanBoxConsts,
    int_primary_vars: &std::collections::BTreeSet<String>,
    float_primary_vars: &std::collections::BTreeSet<String>,
    name: &str,
) -> Value {
    if float_primary_vars.contains(name) {
        *var_get_boxed_overflow_safe_base(
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
        .expect("float escape var not found")
    } else if bool_like_vars.contains(name) {
        ensure_boxed_bool_safe(
            builder,
            vars,
            bool_primary_vars,
            int_primary_vars,
            nbc,
            name,
        )
        .expect("bool variable not found for primitive-safe boxing")
    } else {
        ensure_boxed_overflow_safe(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            int_primary_vars,
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
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    vars: &BTreeMap<String, Variable>,
    int_primary_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    float_primary_vars: &BTreeSet<String>,
    nbc: &crate::NanBoxConsts,
    name: &str,
    boxed: Value,
) {
    if float_primary_vars.contains(name) {
        let raw_f64 =
            float_value_from_boxed_extended(module, import_ids, builder, import_refs, boxed);
        def_var_named(builder, vars, name, raw_f64);
    } else if bool_primary_vars.contains(name) {
        let raw_bool = builder.ins().band_imm(boxed, 1);
        def_var_named(builder, vars, name, raw_bool);
    } else if int_primary_vars.contains(name) {
        let raw_i64 = unbox_int_or_bool(builder, boxed, nbc);
        def_var_named(builder, vars, name, raw_i64);
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
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    vars: &BTreeMap<String, Variable>,
    int_primary_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    float_primary_vars: &BTreeSet<String>,
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
            int_primary_vars,
            bool_primary_vars,
            float_primary_vars,
            nbc,
            name,
            value,
        ),
        ty => panic!("numeric result for {name} has unsupported CLIF type {ty}"),
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn emit_protect_borrowed_args_aliased_return(
    builder: &mut FunctionBuilder<'_>,
    sealed_blocks: &mut BTreeSet<Block>,
    result: Value,
    args: &[Value],
    local_inc_ref_obj: FuncRef,
    nbc: &crate::NanBoxConsts,
) {
    let Some((first, rest)) = args.split_first() else {
        return;
    };
    let mut aliases_arg = builder.ins().icmp(IntCC::Equal, result, *first);
    for arg in rest {
        let aliases_next = builder.ins().icmp(IntCC::Equal, result, *arg);
        aliases_arg = builder.ins().bor(aliases_arg, aliases_next);
    }

    let retain_block = builder.create_block();
    let cont_block = builder.create_block();
    brif_block(builder, aliases_arg, retain_block, &[], cont_block, &[]);

    switch_to_block_materialized(builder, retain_block);
    seal_block_once(builder, sealed_blocks, retain_block);
    emit_inc_ref_obj(builder, result, local_inc_ref_obj, nbc);
    jump_block(builder, cont_block, &[]);

    switch_to_block_materialized(builder, cont_block);
    seal_block_once(builder, sealed_blocks, cont_block);
}

#[cfg(feature = "native-backend")]
#[inline]
pub(in crate::native_backend::function_compiler) fn merge_rebind_storage_for_name(
    name: &str,
    int_primary_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    float_primary_vars: &BTreeSet<String>,
) -> MergeRebindStorageKind {
    if float_primary_vars.contains(name) {
        MergeRebindStorageKind::RawF64
    } else if bool_primary_vars.contains(name) {
        MergeRebindStorageKind::RawBool
    } else if int_primary_vars.contains(name) {
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

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn merge_rebind_value_for_storage(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    bool_like_vars: &BTreeSet<String>,
    int_primary_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    float_primary_vars: &BTreeSet<String>,
    nbc: &crate::NanBoxConsts,
    name: &str,
    storage: MergeRebindStorageKind,
) -> Value {
    match storage {
        MergeRebindStorageKind::RawF64 => float_value_for(builder, vars, float_primary_vars, name)
            .unwrap_or_else(|| {
                let boxed = ensure_boxed_primitive_safe(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    sealed_blocks,
                    bool_like_vars,
                    bool_primary_vars,
                    vars,
                    nbc,
                    int_primary_vars,
                    float_primary_vars,
                    name,
                );
                float_value_from_boxed_extended(module, import_ids, builder, import_refs, boxed)
            }),
        MergeRebindStorageKind::RawI64 => int_raw_value(builder, vars, int_primary_vars, name)
            .or_else(|| bool_raw_value(builder, vars, bool_primary_vars, name))
            .unwrap_or_else(|| {
                let boxed = ensure_boxed_primitive_safe(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    sealed_blocks,
                    bool_like_vars,
                    bool_primary_vars,
                    vars,
                    nbc,
                    int_primary_vars,
                    float_primary_vars,
                    name,
                );
                unbox_int_or_bool(builder, boxed, nbc)
            }),
        MergeRebindStorageKind::RawBool => bool_raw_value(builder, vars, bool_primary_vars, name)
            .or_else(|| int_raw_value(builder, vars, int_primary_vars, name))
            .unwrap_or_else(|| {
                let boxed = ensure_boxed_bool_safe(
                    builder,
                    vars,
                    bool_primary_vars,
                    int_primary_vars,
                    nbc,
                    name,
                )
                .unwrap_or_else(|| {
                    ensure_boxed_primitive_safe(
                        module,
                        import_ids,
                        builder,
                        import_refs,
                        sealed_blocks,
                        bool_like_vars,
                        bool_primary_vars,
                        vars,
                        nbc,
                        int_primary_vars,
                        float_primary_vars,
                        name,
                    )
                });
                builder.ins().band_imm(boxed, 1)
            }),
        MergeRebindStorageKind::BoxedI64 => ensure_boxed_primitive_safe(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            bool_like_vars,
            bool_primary_vars,
            vars,
            nbc,
            int_primary_vars,
            float_primary_vars,
            name,
        ),
    }
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn def_var_from_merge_rebind_storage(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    vars: &BTreeMap<String, Variable>,
    int_primary_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    float_primary_vars: &BTreeSet<String>,
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
            def_raw_bool_value(builder, vars, bool_primary_vars, name, value, nbc);
        }
        MergeRebindStorageKind::BoxedI64 => {
            def_var_from_boxed_transport(
                module,
                import_ids,
                builder,
                import_refs,
                vars,
                int_primary_vars,
                bool_primary_vars,
                float_primary_vars,
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
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    lhs: Value,
    rhs: Value,
    runtime_name: &'static str,
    op: BoxedBitwiseOp,
    nbc: &crate::NanBoxConsts,
) -> Value {
    let callee = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        runtime_name,
        &[types::I64, types::I64],
        &[types::I64],
    );
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
    float_primary_vars: &std::collections::BTreeSet<String>,
    name: &str,
) -> Option<Value> {
    if float_primary_vars.contains(name) {
        return vars.get(name).map(|&var| builder.use_var(var));
    }
    None
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn dead_scrub_value_for_var(
    builder: &mut FunctionBuilder<'_>,
    float_primary_vars: &std::collections::BTreeSet<String>,
    name: &str,
) -> Value {
    if float_primary_vars.contains(name) {
        builder.ins().f64const(0.0)
    } else {
        builder.ins().iconst(types::I64, 0)
    }
}

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) fn float_value_from_boxed_extended(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    boxed: Value,
) -> Value {
    let as_float = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        "molt_float_as_f64",
        &[types::I64],
        &[types::F64],
    );
    let call = builder.ins().call(as_float, &[boxed]);
    builder.inst_results(call)[0]
}

/// Get a raw f64 value for an operand in a mixed float/int arithmetic context.
///
/// This handles the critical case where a float-lane operation has an int
/// operand (e.g. `2.0 * x` where x is int). The resolution order is:
///
///   1. Float-primary Variable (use_var yields f64 directly)
///   2. Int shadow → `fcvt_from_sint` (int-to-float conversion)
///   3. Raw primary int → `fcvt_from_sint`
///   4. NaN-boxed fallback → `unbox_int` + `fcvt_from_sint` for int-like
///      names, runtime extended-float extraction for confirmed floats
///
/// **Critical correctness fix**: earlier code did a raw `bitcast(F64)` for
/// ALL NaN-boxed fallback paths, which reinterprets the NaN-box tag bits as
/// a float value. This produces garbage (NaN) for int operands. Now we
/// correctly convert ints to f64 via `fcvt_from_sint`.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn float_value_for_mixed(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    float_primary_vars: &std::collections::BTreeSet<String>,
    int_primary_vars: &std::collections::BTreeSet<String>,
    int_like_vars: &BTreeSet<String>,
    bool_like_vars: &BTreeSet<String>,
    nbc: &crate::NanBoxConsts,
    name: &str,
) -> Value {
    // 1. Try float-primary path first.
    if let Some(f_val) = float_value_for(builder, vars, float_primary_vars, name) {
        return f_val;
    }

    // 2. Operand is int — get raw i64 and convert to f64.
    if name_is_int_like(name, int_like_vars, bool_like_vars) || int_primary_vars.contains(name) {
        // Phase 1d: int_primary_vars members hold raw i64 in the main
        // Variable; reading via int_raw_value avoids the box/unbox detour.
        if let Some(raw_int_val) = int_raw_value(builder, vars, int_primary_vars, name) {
            return builder.ins().fcvt_from_sint(types::F64, raw_int_val);
        }
        // Fallback: name is int-typed but not in int_primary_vars → vars[name]
        // holds the already-NaN-boxed value. Use overflow-safe boxing so this
        // helper observes the same representation contract as generic reads.
        let boxed = var_get_boxed_overflow_safe_base(
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
        .expect("Int operand not found");
        let raw_int_val = crate::unbox_int(builder, *boxed, nbc);
        return builder.ins().fcvt_from_sint(types::F64, raw_int_val);
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
        int_primary_vars,
        float_primary_vars,
    )
    .expect("Float operand not found");
    float_value_from_boxed_extended(module, import_ids, builder, import_refs, *boxed)
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn emit_float_numeric_compare(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    float_primary_vars: &BTreeSet<String>,
    int_primary_vars: &BTreeSet<String>,
    int_like_vars: &BTreeSet<String>,
    bool_like_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    nbc: &crate::NanBoxConsts,
    out_name: Option<&String>,
    lhs_name: &str,
    rhs_name: &str,
    cc: FloatCC,
) -> (Value, Value) {
    let lhs_f = float_value_for_mixed(
        module,
        import_ids,
        builder,
        import_refs,
        sealed_blocks,
        vars,
        float_primary_vars,
        int_primary_vars,
        int_like_vars,
        bool_like_vars,
        nbc,
        lhs_name,
    );
    let rhs_f = float_value_for_mixed(
        module,
        import_ids,
        builder,
        import_refs,
        sealed_blocks,
        vars,
        float_primary_vars,
        int_primary_vars,
        int_like_vars,
        bool_like_vars,
        nbc,
        rhs_name,
    );
    let cmp = builder.ins().fcmp(cc, lhs_f, rhs_f);
    compare_bool_result_value(builder, bool_primary_vars, out_name, cmp, nbc)
}
