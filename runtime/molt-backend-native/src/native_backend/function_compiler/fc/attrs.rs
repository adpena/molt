use super::super::*;
use crate::tir::simple_def_use::simple_ir_out_result;

/// Single-source kind authority for [`handle_attr_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    // Canonical attribute-op kinds: the spelling `tir::lower_to_simple::lower_op`
    // emits for a LoadAttr/StoreAttr/DelAttr that carries no specialized
    // `_original_kind` — its documented default, the same no-`_original_kind`
    // fallback every other op family already claims (`index`/`store_index`/
    // `del_index`/`call`/`call_builtin`). A TIR pass that yields a generic
    // by-name attribute op produces exactly these (e.g. the cold fallback the
    // release-fast guard-splitting passes leave when they specialize the
    // `guarded_field_get`s in `__future__._Feature.__repr__`). rust/luau/llvm
    // all handle the canonical forms; the native backend must too, or the op
    // hits the dispatch's loud no-codegen catch-all at user `molt build` time.
    // All three get spellings share the bits-validating, tagged-safe boxed
    // setup below; only stable site identity selects the IC ABI.
    "get_attr",
    "get_attr_generic_ptr",
    "get_attr_generic_obj",
    "get_attr_special_obj",
    "load_method",
    "load_super_method",
    "get_attr_name",
    "get_attr_name_default",
    "has_attr_name",
    "set_attr",
    "set_attr_name",
    "set_attr_generic_ptr",
    "set_attr_generic_obj",
    "del_attr",
    "del_attr_generic_ptr",
    "del_attr_generic_obj",
    "del_attr_name",
];
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for object attribute ops: get (`get_attr` (canonical)/`get_attr_generic_ptr`/`_obj`/`_special_obj`/`_name`/`_name_default`), has (`has_attr_name`), set (`set_attr` (canonical)/`set_attr_name`/`_generic_ptr`/`_generic_obj`), and del (`del_attr` (canonical)/`del_attr_generic_ptr`/`_obj`/`_name`). The canonical `get_attr`/`set_attr`/`del_attr` — `tir::lower_to_simple`'s no-`_original_kind` default — share their matching generic boxed lowering.
///
/// Extracted from `compile_func_inner`'s per-op dispatch (M1). Shared setup and
/// authority-equivalent lanes are intentionally consolidated here so new
/// attribute spellings cannot mint backend-local lookup protocols.
/// The op-local closure `var_get_boxed_overflow_safe` preserves the original
/// split-borrow access pattern while the handlers share authority.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_attr_op(
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
) -> OpFlow {
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
        "get_attr" | "get_attr_generic_ptr" | "get_attr_generic_obj" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            let call = if op.kind == "get_attr" {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_get_attr_object",
                    &[types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                builder
                    .ins()
                    .call(local_callee, &[*obj, attr_ptr, attr_len])
            } else {
                let source_op_idx = op.required_source_op_index(op_idx, op.kind.as_str());
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_get_attr_object_ic",
                    &[types::I64, types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let site_bits = builder.ins().iconst(
                    types::I64,
                    box_int(stable_ic_site_id(
                        func_name,
                        source_op_idx,
                        op.kind.as_str(),
                    )),
                );
                builder
                    .ins()
                    .call(local_callee, &[*obj, attr_ptr, attr_len, site_bits])
            };
            let res = builder.inst_results(call)[0];
            // The canonical boxed runtime entrypoints return exactly one owned
            // result on success for every spelling in this branch.
            if let Some(out__) = simple_ir_out_result(op) {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "load_method" | "load_super_method" => {
            // CPython's LOAD_ATTR / LOAD_SUPER_ATTR in method form. The runtime
            // returns the callable and writes `self_or_null` (the receiver or
            // the null sentinel) to a frame slot after every callback it ran.
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let mut operands = Vec::with_capacity(args.len());
            for name in args {
                operands.push(
                    *var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        name,
                        representation_plan,
                    )
                    .unwrap_or_else(|| {
                        panic!("{} operand {name} not found in {func_name}", op.kind)
                    }),
                );
            }
            let (symbol, operand_count) = if op.kind == "load_method" {
                ("molt_load_method", 1)
            } else {
                ("molt_load_super_method", 2)
            };
            assert_eq!(operands.len(), operand_count, "{} operands", op.kind);
            let attr_name = op
                .s_value
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no attribute name", op.kind));
            let data_id = module
                .declare_data(
                    &format!("mname_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();
            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let name_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let name_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            let source_op_idx = op.required_source_op_index(op_idx, op.kind.as_str());
            let site_bits = builder.ins().iconst(
                types::I64,
                box_int(stable_ic_site_id(
                    func_name,
                    source_op_idx,
                    op.kind.as_str(),
                )),
            );
            let self_slot = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                8,
                3,
            ));
            // The slot holds the null sentinel until the runtime writes it, so
            // a call that raises before its write leaves a releasable word.
            let null = builder
                .ins()
                .iconst(types::I64, molt_codegen_abi::loaded_method_null_bits());
            builder.ins().stack_store(null, self_slot, 0);
            let self_out = builder.ins().stack_addr(types::I64, self_slot, 0);
            let mut call_args = vec![site_bits];
            call_args.extend_from_slice(&operands);
            call_args.extend_from_slice(&[name_ptr, name_len, self_out]);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                symbol,
                &vec![types::I64; call_args.len()],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &call_args);
            let callable = builder.inst_results(call)[0];
            let self_or_null = builder.ins().stack_load(types::I64, self_slot, 0);
            use crate::tir::simple_def_use::{SimpleIrResultField, visit_simple_ir_results};
            let mut results = [None, None];
            visit_simple_ir_results(op, |result| match result.field {
                SimpleIrResultField::Var => results[0] = result.name,
                SimpleIrResultField::Out => results[1] = result.name,
                SimpleIrResultField::Arg(_) => unreachable!("a method load has fixed result roles"),
            });
            for (name, bits) in results.into_iter().zip([callable, self_or_null]) {
                match name {
                    Some(name) => def_var_from_boxed_transport(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        vars,
                        representation_plan,
                        nbc,
                        name,
                        bits,
                    ),
                    // A discarded word is owned; releasing the null sentinel is a no-op.
                    None => bind_owned_runtime_result_name(
                        None,
                        bits,
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        vars,
                    ),
                }
            }
        }
        "get_attr_special_obj" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_get_attr_special",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*obj, attr_ptr, attr_len]);
            let res = builder.inst_results(call)[0];
            // `molt_get_attr_special` returns one owned result on success.
            if let Some(out__) = simple_ir_out_result(op) {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "get_attr_name" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let name = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr name not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_get_attr_name",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj, *name]);
            let res = builder.inst_results(call)[0];
            // `molt_get_attr_name` returns one owned result on success.
            if let Some(out__) = simple_ir_out_result(op) {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "get_attr_name_default" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let name = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr name not found");
            let default = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Attr default not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_get_attr_name_default",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj, *name, *default]);
            let res = builder.inst_results(call)[0];
            // `molt_get_attr_name_default` owns both lookup and default results.
            if let Some(out__) = simple_ir_out_result(op) {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "has_attr_name" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let name = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr name not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_has_attr_name",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj, *name]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = simple_ir_out_result(op) {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "set_attr_name" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let name = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr name not found");
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Attr value not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_set_attr_name",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj, *name, *val]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = simple_ir_out_result(op) {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "set_attr" | "set_attr_generic_ptr" | "set_attr_generic_obj" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr value not found");
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            // The receiver stays NaN-boxed: a polymorphic receiver can be a
            // tagged non-pointer (`typing.final(42)` then `f.__final__ = True`),
            // and both entries resolve it with `maybe_ptr_from_bits`, raising a
            // catchable AttributeError instead of reading a header at a tag.
            let call = if op.kind == "set_attr" {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_set_attr_object",
                    &[types::I64, types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                builder
                    .ins()
                    .call(local_callee, &[*obj, attr_ptr, attr_len, *val])
            } else {
                let source_op_idx = op.required_source_op_index(op_idx, op.kind.as_str());
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_set_attr_object_ic",
                    &[types::I64, types::I64, types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let site_bits = builder.ins().iconst(
                    types::I64,
                    box_int(stable_ic_site_id(
                        func_name,
                        source_op_idx,
                        op.kind.as_str(),
                    )),
                );
                builder
                    .ins()
                    .call(local_callee, &[*obj, attr_ptr, attr_len, *val, site_bits])
            };
            if let Some(out_name) = simple_ir_out_result(op) {
                let res = builder.inst_results(call)[0];
                def_var_named(&mut *builder, vars, out_name, res);
            }
        }
        "del_attr_generic_ptr" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            // Pass the NaN-boxed receiver and call the bits-validating
            // `molt_del_attr_object` (mirrors the `set_attr_generic_ptr` fix
            // above): the `_generic_ptr` DELATTR form can target a tagged
            // non-pointer receiver, and `unbox_ptr_value` of a tagged value
            // followed by `molt_del_attr_ptr`'s header deref would SIGSEGV.
            // `molt_del_attr_object` resolves via `maybe_ptr_from_bits` and raises
            // a clean AttributeError/TypeError for a tagged receiver, with no
            // behavior change for real heap objects.
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_del_attr_object",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*obj, attr_ptr, attr_len]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = simple_ir_out_result(op) {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "del_attr" | "del_attr_generic_obj" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_del_attr_object",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*obj, attr_ptr, attr_len]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = simple_ir_out_result(op) {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "del_attr_name" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Attr object not found in {} op {}", func_name, op_idx));
            let name = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Attr name not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_del_attr_name",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj, *name]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = simple_ir_out_result(op) {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
    OpFlow::Proceed
}
