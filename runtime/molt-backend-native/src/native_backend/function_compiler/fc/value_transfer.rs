use super::super::*;

/// Single-source kind authority for [`handle_value_transfer_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "inc_ref",
    "borrow",
    "dec_ref",
    "del_boundary",
    "release",
    "box",
    "unbox",
    "cast",
    "widen",
    "identity_alias",
    "binding_alias",
    "copy",
];
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for value-custody transfer ops: `inc_ref`,
/// `borrow`, `dec_ref`, `del_boundary`, `release`, `box`, `unbox`, `cast`,
/// `widen`, and retained alias ops. This owns alias-preserving refcount
/// adjustment and path-local owner-token consumption for explicit releases.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_value_transfer_op(
    op: &OpIR,
    op_idx: usize,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    alias_roots: &BTreeMap<String, String>,
    cleanup_roots: &mut NativeCleanupRoots,
    rc_skip_inc: &std::collections::HashSet<usize>,
    rc_authority: NativeRcAuthority,
    local_inc_ref_obj: FuncRef,
    local_dec_ref_obj: FuncRef,
    nbc: &crate::NanBoxConsts,
) {
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
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };

    match op.kind.as_str() {
        "inc_ref" | "borrow" => {
            if !rc_skip_inc.contains(&op_idx) {
                let args_names = op.args.as_ref().expect("inc_ref/borrow args missing");
                let src_name = args_names
                    .first()
                    .expect("inc_ref/borrow requires one source arg");
                let src = *var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    src_name,
                    representation_plan,
                )
                .expect("inc_ref/borrow source not found");
                emit_inc_ref_obj(&mut *builder, src, local_inc_ref_obj);
                if op.out.as_deref().is_none_or(|name| name == "none") {
                    cleanup_roots.retain_explicit(builder, src_name);
                }
                if let Some(out_name) = op.out.as_ref()
                    && out_name != "none"
                {
                    def_var_named(&mut *builder, vars, out_name.clone(), src);
                }
            } else if let Some(out_name) = op.out.as_ref()
                && out_name != "none"
            {
                // RC coalesced: still define the output variable as an
                // alias of the input so downstream ops can read it.
                let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                let src_name = args_names.first().unwrap();
                let src = *var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    src_name,
                    representation_plan,
                )
                .expect("inc_ref/borrow source not found (coalesced)");
                def_var_named(&mut *builder, vars, out_name.clone(), src);
            }
        }
        "dec_ref" | "release" => {
            let args_names = op.args.as_ref().expect("dec_ref/release args missing");
            let src_name = args_names
                .first()
                .expect("dec_ref/release requires one source arg");
            if rc_skip_inc.contains(&op_idx) {
                // No runtime call needed.  Still define the output
                // variable so downstream SSA reads succeed.
                if let Some(out_name) = op.out.as_ref()
                    && out_name != "none"
                {
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    def_var_named(&mut *builder, vars, out_name.clone(), none_bits);
                }
            } else {
                let src = *var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    src_name,
                    representation_plan,
                )
                .expect("dec_ref/release source not found");
                cleanup_roots.consume_explicit(builder, src_name);
                builder.ins().call(local_dec_ref_obj, &[src]);
                if let Some(out_name) = op.out.as_ref()
                    && out_name != "none"
                {
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    def_var_named(&mut *builder, vars, out_name.clone(), none_bits);
                }
            }
        }
        "del_boundary" => {
            // Native preanalysis consumes DelBoundary to pin Python lifetime
            // boundaries. Drop insertion normally normalizes it away; if it
            // survives on a dormant-native lane, codegen must route it
            // explicitly and perform no second release here.
        }
        // `copy` is the frontend's args-based pure SSA value move
        // (`{kind:"copy", args:[src], out:result}`). It survives
        // `rewrite_copy_aliases` whenever its result/source is a mutable-storage
        // (reassigned-local) name, so it reaches codegen and must be lowered
        // here rather than silently dropped. It shares the alias lowering:
        // Generated TIR facts distinguish transparent bit-passthrough aliases
        // from the sole alias kind that mints a new owned reference. Keep
        // Cranelift aligned with TIR, WASM, and LIR-fast: `copy` and
        // `identity_alias` share their source root; `binding_alias` contributes
        // exactly +1. Direct conversions use the same representation boundary:
        // boxed inputs retain an independent result; raw inputs materialize
        // their result ownership without retaining a nonexistent source owner.
        "copy" | "identity_alias" | "binding_alias" | "box" | "unbox" | "cast" | "widen" => {
            let args_names = op.args.as_ref().expect("alias args missing");
            let src_name = args_names
                .first()
                .expect("alias op requires one source arg");
            if let Some(out_name) = op.out.as_ref()
                && out_name != "none"
            {
                // The unboxed-scalar primary lanes each carry a RAW
                // machine value in the destination's Cranelift Variable — raw
                // i64, raw 0/1, raw f64 respectively (see `int_raw_value` /
                // `bool_raw_value` / `float_value_for`). An alias whose OUT is a
                // primary-lane carrier must therefore transfer the RAW value, NOT
                // a NaN-boxed value: storing a boxed value into a raw-lane
                // Variable makes every downstream raw read reinterpret the
                // NaN-box bits as a scalar (the chained-init `a = b = 0` →
                // float-accumulator freeze: `b`'s `binding_alias` seed landed
                // boxed in an int-primary slot, so the loop carried garbage).
                // Unboxed scalars are not heap objects, so no inc_ref is taken
                // (mirroring the raw-scalar arms of `merge_rebind_value_for_storage`).
                if representation_plan.is_float_unboxed(out_name) {
                    let raw_f64 =
                        float_value_for(&mut *builder, vars, representation_plan, src_name)
                            .unwrap_or_else(|| {
                                let boxed = var_get_boxed_overflow_safe(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    &mut *import_refs,
                                    &mut *sealed_blocks,
                                    vars,
                                    src_name,
                                    representation_plan,
                                )
                                .expect("alias source not found");
                                float_value_from_boxed_extended(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    &mut *import_refs,
                                    *boxed,
                                )
                            });
                    def_var_named(&mut *builder, vars, out_name.clone(), raw_f64);
                } else if representation_plan.is_raw_int_carrier_name(out_name) {
                    // Int-primary: transfer raw i64 directly.
                    let raw_i64 = int_raw_value(&mut *builder, vars, representation_plan, src_name)
                        .or_else(|| {
                            bool_raw_value(&mut *builder, vars, representation_plan, src_name)
                        })
                        .unwrap_or_else(|| {
                            let boxed = var_get_boxed_overflow_safe(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                &mut *import_refs,
                                &mut *sealed_blocks,
                                vars,
                                src_name,
                                representation_plan,
                            )
                            .expect("alias source not found");
                            unbox_int_or_bool(&mut *builder, *boxed, nbc)
                        });
                    def_var_named(&mut *builder, vars, out_name.clone(), raw_i64);
                } else if representation_plan.is_bool_unboxed(out_name) {
                    // Bool-primary: transfer raw 0/1 directly.
                    let raw_bool =
                        bool_raw_value(&mut *builder, vars, representation_plan, src_name)
                            .or_else(|| {
                                int_raw_value(&mut *builder, vars, representation_plan, src_name)
                            })
                            .unwrap_or_else(|| {
                                let boxed = var_get_boxed_overflow_safe(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    &mut *import_refs,
                                    &mut *sealed_blocks,
                                    vars,
                                    src_name,
                                    representation_plan,
                                )
                                .expect("alias source not found");
                                unbox_int_or_bool(&mut *builder, *boxed, nbc)
                            });
                    def_var_named(&mut *builder, vars, out_name.clone(), raw_bool);
                } else {
                    let src = *var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        src_name,
                        representation_plan,
                    )
                    .expect("alias source not found");
                    let kind = op.kind.as_str();
                    let retained_binding =
                        crate::tir::op_kinds_generated::copy_kind_mints_owned_alias_ref_table(kind)
                            || (rc_authority.native_value_tracking_enabled()
                                && (matches!(kind, "box" | "unbox" | "cast" | "widen")
                                    || native_alias_mints_owner(alias_roots, src_name, out_name)));
                    if retained_binding
                        && merge_rebind_storage_for_name(src_name, representation_plan)
                            == MergeRebindStorageKind::BoxedI64
                    {
                        emit_inc_ref_obj(builder, src, local_inc_ref_obj);
                    }
                    def_var_named(&mut *builder, vars, out_name.clone(), src);
                }
            }
        }
        _ => unreachable!("non-value-transfer op routed to handle_value_transfer_op"),
    }
}
