use super::super::*;
use super::var_get_boxed_overflow_safe_fn;

/// Single-source kind authority for [`handle_parse_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] =
    &["json_parse", "msgpack_parse", "cbor_parse"];

/// Cranelift codegen handlers for structured-data parsers.
///
/// Parser inputs are ordinary boxed objects. The object providers own all
/// decoding and input representation details; lowering must not infer raw
/// pointer/length companions from unrelated SSA names. A raw integer carrier
/// is a temporary owned boxing escape: release it on both paths and do not call
/// the provider when boxing raises.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_parse_op(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
) {
    match op.kind.as_str() {
        kind @ ("json_parse" | "msgpack_parse" | "cbor_parse") => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let arg_name = &args[0];
            let origin = builder.current_block();
            let owns_temporary = representation_plan.is_raw_int_carrier_name(arg_name);
            let arg_bits = var_get_boxed_overflow_safe_fn(
                module,
                import_ids,
                builder,
                import_refs,
                sealed_blocks,
                vars,
                arg_name,
                representation_plan,
                nbc,
            )
            .unwrap_or_else(|| panic!("{kind} input `{arg_name}` not found"));
            let symbol = match kind {
                "json_parse" => "molt_json_parse_scalar_obj",
                "msgpack_parse" => "molt_msgpack_parse_scalar_obj",
                "cbor_parse" => "molt_cbor_parse_scalar_obj",
                _ => unreachable!("outer match restricts the parser family"),
            };
            let callee = SimpleBackend::import_func_id_split(
                module,
                import_ids,
                symbol,
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            if owns_temporary {
                let release = import_func_ref(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    "molt_dec_ref_obj",
                    &[types::I64],
                    &[],
                );
                let pending = import_func_ref(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    "molt_exception_pending_fast",
                    &[],
                    &[types::I64],
                );
                let invoke = builder.create_block();
                let abort = builder.create_block();
                builder.set_cold_block(abort);
                let merge = builder.create_block();
                builder.append_block_param(merge, types::I64);
                let failed = emit_exception_pending_condition(builder, pending, None);
                builder.ins().brif(failed, abort, &[], invoke, &[]);

                switch_to_block_materialized(builder, invoke);
                seal_block_once(builder, sealed_blocks, invoke);
                let call = builder.ins().call(local_callee, &[*arg_bits]);
                let result = builder.inst_results(call)[0];
                builder.ins().call(release, &[*arg_bits]);
                jump_block(builder, merge, &[result]);

                switch_to_block_materialized(builder, abort);
                seal_block_once(builder, sealed_blocks, abort);
                builder.ins().call(release, &[*arg_bits]);
                let none = builder.ins().iconst(types::I64, box_none());
                jump_block(builder, merge, &[none]);

                switch_to_block_materialized(builder, merge);
                seal_block_once(builder, sealed_blocks, merge);
                let result = builder.block_params(merge)[0];
                bind_owned_runtime_result(op, result, module, import_ids, builder, vars);
                carry_internal_cfg_tracking(origin, merge, block_tracked_obj, block_tracked_ptr);
            } else {
                let call = builder.ins().call(local_callee, &[*arg_bits]);
                let result = builder.inst_results(call)[0];
                bind_owned_runtime_result(op, result, module, import_ids, builder, vars);
            }
        }
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
}
