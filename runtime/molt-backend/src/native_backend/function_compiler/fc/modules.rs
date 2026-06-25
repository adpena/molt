use super::super::*;

/// Single-source kind authority for [`handle_module_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "module_new",
    "module_cache_get",
    "module_import",
    "module_cache_set",
    "module_cache_del",
    "module_get_attr",
    "module_import_from",
    "module_get_global",
    "module_del_global",
    "module_del_global_if_present",
    "module_get_name",
    "module_set_attr",
    "module_import_star",
];
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for module ops: creation/import (`module_new`/`module_import`/`module_import_star`), cache (`cache_get`/`cache_set`/`cache_del`), attribute/global access (`get_attr`/`import_from`/`get_global`/`del_global`/`set_attr`/`get_name`).
///
/// Extracted verbatim from `compile_func_inner`'s per-op dispatch (M1).
/// Each arm body is byte-for-byte identical to the original; only the access
/// path to the backend's split-borrowed fields changed (`self.module` ->
/// `module`, `Self::` -> `SimpleBackend::`, owned locals -> reborrowed params).
/// The op-local closure `var_get_boxed_overflow_safe` is reconstructed with the
/// same capture so the arm bodies are unchanged.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_module_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
    local_inc_ref_obj: FuncRef,
    hoisted_str_slot: &BTreeMap<String, cranelift_codegen::ir::StackSlot>,
) {
    // Reconstruct the original op-local closure (captures representation_plan +
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
        "module_new" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let name_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Module name not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_module_new",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*name_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "module_cache_get" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let name_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Module name not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_module_cache_get",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*name_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "module_import" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let name_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Module name not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_module_import",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*name_bits]);
            let res = builder.inst_results(call)[0];
            // module_import may return a borrowed reference from sys.modules —
            // inc_ref to ensure the caller owns it and dec_ref at last_use
            // doesn't free a module still in sys.modules.
            emit_maybe_ref_adjust_v2(&mut *builder, res, local_inc_ref_obj, nbc);
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "module_cache_set" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let name_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Module name not found");
            let module_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Module not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_module_cache_set",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder
                .ins()
                .call(local_callee, &[*name_bits, *module_bits]);
        }
        "module_cache_del" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let name_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Module name not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_module_cache_del",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder.ins().call(local_callee, &[*name_bits]);
        }
        "module_get_attr" | "module_import_from" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let module_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| {
                panic!(
                    "Module not found in {} op {} ({:?})",
                    func_name, op_idx, op.args
                )
            });
            // Load attr name from stack slot if this is a const_str.
            let _has = hoisted_str_slot.contains_key(&args[1]);
            if std::env::var("MOLT_DEBUG_MODULE_GET_ATTR").as_deref() == Ok("1") {
                let _ = crate::debug_artifacts::append_debug_artifact(
                    "native/module_get_attr_diag.txt",
                    format!(
                        "mga: func={} op={} arg1={} has_slot={} slot_count={}\n",
                        func_name,
                        op_idx,
                        &args[1],
                        _has,
                        hoisted_str_slot.len()
                    ),
                );
            }
            let attr_val = if let Some(&slot) = hoisted_str_slot.get(&args[1]) {
                builder.ins().stack_load(types::I64, slot, 0)
            } else {
                *var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[1],
                    representation_plan,
                )
                .unwrap_or_else(|| {
                    panic!(
                        "Attr not found in {} op {} ({:?})",
                        func_name, op_idx, op.args
                    )
                })
            };
            // `from M import name` routes to molt_module_import_from
            // (CPython IMPORT_FROM semantics: ImportError on miss with a
            // sys.modules submodule fallback); plain `M.name` reads use
            // molt_module_get_attr (AttributeError on miss).
            let runtime_symbol = if op.kind == "module_import_from" {
                "molt_module_import_from"
            } else {
                "molt_module_get_attr"
            };
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                runtime_symbol,
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*module_bits, attr_val]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "module_get_global" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let module_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Module not found");
            let attr_bits = *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_module_get_global",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*module_bits, attr_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "module_del_global" | "module_del_global_if_present" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let module_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Module not found");
            let attr_bits = *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr not found");
            let callee_name = if op.kind == "module_del_global_if_present" {
                "molt_module_del_global_if_present"
            } else {
                "molt_module_del_global"
            };
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                callee_name,
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*module_bits, attr_bits]);
            if let Some(out_name) = op.out.as_ref()
                && out_name != "none"
            {
                let res = builder.inst_results(call)[0];
                def_var_named(&mut *builder, vars, out_name.clone(), res);
            }
        }
        "module_get_name" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let module_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Module not found");
            let attr_bits = *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_module_get_name",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*module_bits, attr_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "module_set_attr" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let module_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Module not found");
            let attr_bits = if let Some(&slot) = hoisted_str_slot.get(&args[1]) {
                builder.ins().stack_load(types::I64, slot, 0)
            } else {
                *var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[1],
                    representation_plan,
                )
                .expect("Attr not found")
            };
            let val_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .unwrap_or_else(|| {
                panic!(
                    "Value not found for module_set_attr in {} op {}",
                    func_name, op_idx
                )
            });
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_module_set_attr",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder
                .ins()
                .call(local_callee, &[*module_bits, attr_bits, *val_bits]);
        }
        "module_import_star" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let src_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Module not found");
            let dst_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Module not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_module_import_star",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder.ins().call(local_callee, &[*src_bits, *dst_bits]);
        }
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
}
