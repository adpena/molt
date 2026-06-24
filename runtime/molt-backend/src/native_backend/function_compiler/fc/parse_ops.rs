use super::super::*;

/// Single-source kind authority for [`handle_parse_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] =
    &["json_parse", "msgpack_parse", "cbor_parse"];
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for in-process structured-data parsers: `json_parse`/`msgpack_parse`/`cbor_parse`.
///
/// Extracted verbatim from `compile_func_inner`'s per-op dispatch (M1).
/// Each arm body is byte-for-byte identical to the original; only the access
/// path to the backend's split-borrowed fields changed (`self.module` ->
/// `module`, `Self::` -> `SimpleBackend::`, owned locals -> reborrowed params).
/// The op-local closure `var_get_boxed_overflow_safe` is reconstructed with the
/// same capture so the arm bodies are unchanged.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_parse_op(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    int_primary_vars: &BTreeSet<String>,
    float_primary_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    nbc: &crate::NanBoxConsts,
) {
    // Reconstruct the original op-local closure (captures bool_primary_vars +
    // nbc; all other state threads through explicit params) so the moved arm
    // bodies call it exactly as they did inline.
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
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            int_primary_vars,
            float_primary_vars,
            bool_primary_vars,
            nbc,
        )
    };
    match op.kind.as_str() {
        "json_parse" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let arg_name = &args[0];
            if let Some(len) = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &format!("{}_len", arg_name),
                int_primary_vars,
                float_primary_vars,
            ) {
                let ptr = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &format!("{}_ptr", arg_name),
                    int_primary_vars,
                    float_primary_vars,
                )
                .or_else(|| {
                    var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        arg_name,
                        int_primary_vars,
                        float_primary_vars,
                    )
                })
                .expect("String ptr not found");

                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_json_parse_scalar",
                    &[types::I64, types::I64, types::I64],
                    &[types::I32],
                );
                let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*ptr, *len, out_ptr]);
                let rc = builder.inst_results(call)[0];
                let ok_block = builder.create_block();
                let err_block = builder.create_block();
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);
                let ok = builder.ins().icmp_imm(IntCC::Equal, rc, 0);
                brif_block(&mut *builder, ok, ok_block, &[], err_block, &[]);

                switch_to_block_materialized(&mut *builder, ok_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, ok_block);
                let ok_res = builder
                    .ins()
                    .load(types::I64, MemFlags::trusted(), out_ptr, 0);
                jump_block(&mut *builder, merge_block, &[ok_res]);

                switch_to_block_materialized(&mut *builder, err_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, err_block);
                let arg_bits = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    arg_name,
                    int_primary_vars,
                    float_primary_vars,
                )
                .expect("String arg not found");
                let err_callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_json_parse_scalar_obj",
                    &[types::I64],
                    &[types::I64],
                );
                let err_local = module.declare_func_in_func(err_callee, builder.func);
                let err_call = builder.ins().call(err_local, &[*arg_bits]);
                let err_res = builder.inst_results(err_call)[0];
                jump_block(&mut *builder, merge_block, &[err_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                let res = builder.block_params(merge_block)[0];
                if let Some(out__) = op.out.as_ref() {
                    def_var_named(&mut *builder, vars, out__, res);
                }
            } else {
                let arg_bits = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    arg_name,
                    int_primary_vars,
                    float_primary_vars,
                )
                .expect("String arg not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_json_parse_scalar_obj",
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*arg_bits]);
                let res = builder.inst_results(call)[0];
                if let Some(out__) = op.out.as_ref() {
                    def_var_named(&mut *builder, vars, out__, res);
                }
            }
        }
        "msgpack_parse" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let arg_name = &args[0];
            if let Some(len) = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &format!("{}_len", arg_name),
                int_primary_vars,
                float_primary_vars,
            ) {
                let ptr = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &format!("{}_ptr", arg_name),
                    int_primary_vars,
                    float_primary_vars,
                )
                .or_else(|| {
                    var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        arg_name,
                        int_primary_vars,
                        float_primary_vars,
                    )
                })
                .expect("Bytes ptr not found");

                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_msgpack_parse_scalar",
                    &[types::I64, types::I64, types::I64],
                    &[types::I32],
                );
                let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*ptr, *len, out_ptr]);
                let rc = builder.inst_results(call)[0];
                let ok_block = builder.create_block();
                let err_block = builder.create_block();
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);
                let ok = builder.ins().icmp_imm(IntCC::Equal, rc, 0);
                brif_block(&mut *builder, ok, ok_block, &[], err_block, &[]);

                switch_to_block_materialized(&mut *builder, ok_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, ok_block);
                let ok_res = builder
                    .ins()
                    .load(types::I64, MemFlags::trusted(), out_ptr, 0);
                jump_block(&mut *builder, merge_block, &[ok_res]);

                switch_to_block_materialized(&mut *builder, err_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, err_block);
                let arg_bits = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    arg_name,
                    int_primary_vars,
                    float_primary_vars,
                )
                .expect("Bytes arg not found");
                let err_callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_msgpack_parse_scalar_obj",
                    &[types::I64],
                    &[types::I64],
                );
                let err_local = module.declare_func_in_func(err_callee, builder.func);
                let err_call = builder.ins().call(err_local, &[*arg_bits]);
                let err_res = builder.inst_results(err_call)[0];
                jump_block(&mut *builder, merge_block, &[err_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                let res = builder.block_params(merge_block)[0];
                if let Some(out__) = op.out.as_ref() {
                    def_var_named(&mut *builder, vars, out__, res);
                }
            } else {
                let arg_bits = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    arg_name,
                    int_primary_vars,
                    float_primary_vars,
                )
                .expect("Bytes arg not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_msgpack_parse_scalar_obj",
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*arg_bits]);
                let res = builder.inst_results(call)[0];
                if let Some(out__) = op.out.as_ref() {
                    def_var_named(&mut *builder, vars, out__, res);
                }
            }
        }
        "cbor_parse" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let arg_name = &args[0];
            if let Some(len) = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &format!("{}_len", arg_name),
                int_primary_vars,
                float_primary_vars,
            ) {
                let ptr = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &format!("{}_ptr", arg_name),
                    int_primary_vars,
                    float_primary_vars,
                )
                .or_else(|| {
                    var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        arg_name,
                        int_primary_vars,
                        float_primary_vars,
                    )
                })
                .expect("Bytes ptr not found");

                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_cbor_parse_scalar",
                    &[types::I64, types::I64, types::I64],
                    &[types::I32],
                );
                let out_slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let out_ptr = builder.ins().stack_addr(types::I64, out_slot, 0);
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*ptr, *len, out_ptr]);
                let rc = builder.inst_results(call)[0];
                let ok_block = builder.create_block();
                let err_block = builder.create_block();
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);
                let ok = builder.ins().icmp_imm(IntCC::Equal, rc, 0);
                brif_block(&mut *builder, ok, ok_block, &[], err_block, &[]);

                switch_to_block_materialized(&mut *builder, ok_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, ok_block);
                let ok_res = builder
                    .ins()
                    .load(types::I64, MemFlags::trusted(), out_ptr, 0);
                jump_block(&mut *builder, merge_block, &[ok_res]);

                switch_to_block_materialized(&mut *builder, err_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, err_block);
                let arg_bits = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    arg_name,
                    int_primary_vars,
                    float_primary_vars,
                )
                .expect("Bytes arg not found");
                let err_callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_cbor_parse_scalar_obj",
                    &[types::I64],
                    &[types::I64],
                );
                let err_local = module.declare_func_in_func(err_callee, builder.func);
                let err_call = builder.ins().call(err_local, &[*arg_bits]);
                let err_res = builder.inst_results(err_call)[0];
                jump_block(&mut *builder, merge_block, &[err_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                let res = builder.block_params(merge_block)[0];
                if let Some(out__) = op.out.as_ref() {
                    def_var_named(&mut *builder, vars, out__, res);
                }
            } else {
                let arg_bits = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    arg_name,
                    int_primary_vars,
                    float_primary_vars,
                )
                .expect("Bytes arg not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_cbor_parse_scalar_obj",
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*arg_bits]);
                let res = builder.inst_results(call)[0];
                if let Some(out__) = op.out.as_ref() {
                    def_var_named(&mut *builder, vars, out__, res);
                }
            }
        }
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
}
