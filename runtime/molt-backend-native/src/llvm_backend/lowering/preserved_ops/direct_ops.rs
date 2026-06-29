use super::*;

/// Handler-owned authority for LLVM preserved SimpleIR ops lowered directly by
/// this module. The dispatcher in `preserved_ops.rs` routes through this slice;
/// audit tooling compares it with the adjacent `match kind` arms so routing and
/// lowering cannot drift independently.
pub(super) const HANDLED_KINDS: &[&str] = &[
    "call_async",
    "chan_new",
    "cast",
    "widen",
    "copy_var",
    "aiter",
    "gen_send",
    "context_exit",
    "super_new",
    "class_def",
    "module_new",
    "module_cache_get",
    "module_cache_set",
    "module_cache_del",
    "module_get_attr",
    "module_import_from",
    "module_get_global",
    "module_set_attr",
    "module_del_global",
    "module_del_global_if_present",
    "exception_class",
    "exception_new",
    "exception_new_builtin",
    "exception_new_builtin_empty",
    "exception_new_builtin_one",
    "exception_push",
    "exception_stack_enter",
    "exception_stack_depth",
    "exception_stack_set_depth",
    "exception_stack_exit",
    "exception_pop",
    "exception_stack_clear",
    "exception_last",
    "exception_last_pending",
    "exception_finally_pending_observer",
    "exception_active",
    "exception_current",
    "exception_enter_handler",
    "exception_resolve_captured",
    "exception_clear",
    "exception_set_last",
    "exception_context_set",
    "builtin_type",
    "class_apply_set_name",
    "class_layout_version",
    "class_set_layout_version",
    "object_set_class",
    "class_merge_layout",
    "str_from_obj",
    "repr_from_obj",
    "int_from_obj",
    "float_from_obj",
    "string_format",
    "ascii_from_obj",
    "complex_from_obj",
    "object_new",
    "int_from_str_of_obj",
    "ord",
    "ord_at",
    "string_join",
    "isinstance",
    "exception_match_builtin",
    "issubclass",
    "has_attr_name",
    "type_of",
    "missing",
    "is_callable",
    "get_attr_name_default",
    "context_depth",
    "context_unwind_to",
    "dataclass_new",
    "dataclass_new_values",
    "abs",
    "const_ellipsis",
    "const_not_implemented",
    "gen_throw",
    "gen_close",
    "exception_set_cause",
    "get_attr_special_obj",
    "borrow",
    "identity_alias",
    "binding_alias",
    "release",
    "gen_locals_register",
    "guard_type",
    "guard_tag",
    "guard_layout",
    "guard_dict_shape",
    "guarded_field_init",
    "json_parse",
    "msgpack_parse",
    "cbor_parse",
    "floordiv",
    "invert",
    "contains",
    "inplace_bit_and",
    "inplace_bit_or",
    "inplace_bit_xor",
    "inplace_div",
    "inplace_floordiv",
    "inplace_mod",
    "inplace_pow",
    "inplace_lshift",
    "inplace_rshift",
];

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn lower_preserved_direct_op(&mut self, op: &TirOp, kind: &str) -> bool {
        let i64_ty = self.backend.context.i64_type();
        match kind {
            "call_async" => self.lower_preserved_call_async_op(op),
            "chan_new" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_chan_new", 1);
                let capacity_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[capacity_bits.into()], "chan_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // Repr-identity SimpleIR ops. Native and WASM lower these as
            // operand-0 passthroughs over the same NaN-boxed value format; LLVM
            // must claim the exact same identity fact explicitly so the terminal
            // preserved-op guard remains a true fail-loud backstop rather than a
            // backend skew. No runtime call, ownership transfer, or new value is
            // introduced here.
            "cast" | "widen" | "copy_var" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let src_val = self.resolve(src_id);
                let src_ty = self
                    .value_types
                    .get(&src_id)
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                for &result_id in &op.results {
                    self.values.insert(result_id, src_val);
                    self.value_types.insert(result_id, src_ty.clone());
                }
                true
            }
            "aiter" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_aiter", 1);
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[obj_bits.into()], "aiter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "gen_send" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_generator_send", 2);
                let gen_bits = self.materialize_dynbox_operand(op.operands[0]);
                let value_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[gen_bits.into(), value_bits.into()], "gen_send")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "context_exit" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_context_exit", 2);
                let ctx_bits = self.materialize_dynbox_operand(op.operands[0]);
                let exc_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[ctx_bits.into(), exc_bits.into()], "context_exit")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "super_new" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let func = self.ensure_runtime_i64_fn("molt_super_new", 2);
                let type_bits = self.materialize_dynbox_operand(op.operands[0]);
                let obj_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(func, &[type_bits.into(), obj_bits.into()], "super_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "class_def" => {
                let Some(meta) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) else {
                    return false;
                };
                let mut parts = meta.split(',');
                let Some(nbases) = parts.next().and_then(|s| s.parse::<usize>().ok()) else {
                    return false;
                };
                let Some(nattrs) = parts.next().and_then(|s| s.parse::<usize>().ok()) else {
                    return false;
                };
                let Some(layout_size) = parts.next().and_then(|s| s.parse::<i64>().ok()) else {
                    return false;
                };
                let Some(layout_version) = parts.next().and_then(|s| s.parse::<i64>().ok()) else {
                    return false;
                };
                let Some(flags) = parts.next().and_then(|s| s.parse::<i64>().ok()) else {
                    return false;
                };
                if op.operands.is_empty() || op.operands.len() != 1 + nbases + nattrs * 2 {
                    return false;
                }
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let bases_count = nbases.max(1) as u64;
                let attrs_count = (nattrs * 2).max(1) as u64;
                let bases_alloca = self
                    .backend
                    .builder
                    .build_array_alloca(i64_ty, i64_ty.const_int(bases_count, false), "class_bases")
                    .unwrap();
                let attrs_alloca = self
                    .backend
                    .builder
                    .build_array_alloca(i64_ty, i64_ty.const_int(attrs_count, false), "class_attrs")
                    .unwrap();
                for idx in 0..nbases {
                    let base_bits = self.materialize_dynbox_operand(op.operands[1 + idx]);
                    let elem_ptr = unsafe {
                        self.backend
                            .builder
                            .build_gep(
                                i64_ty,
                                bases_alloca,
                                &[i64_ty.const_int(idx as u64, false)],
                                &format!("class_base_ptr_{idx}"),
                            )
                            .unwrap()
                    };
                    self.backend
                        .builder
                        .build_store(elem_ptr, base_bits)
                        .unwrap();
                }
                let attrs_start = 1 + nbases;
                for idx in 0..(nattrs * 2) {
                    let value_bits =
                        self.materialize_dynbox_operand(op.operands[attrs_start + idx]);
                    let elem_ptr = unsafe {
                        self.backend
                            .builder
                            .build_gep(
                                i64_ty,
                                attrs_alloca,
                                &[i64_ty.const_int(idx as u64, false)],
                                &format!("class_attr_ptr_{idx}"),
                            )
                            .unwrap()
                    };
                    self.backend
                        .builder
                        .build_store(elem_ptr, value_bits)
                        .unwrap();
                }
                let bases_ptr_bits = self
                    .backend
                    .builder
                    .build_ptr_to_int(bases_alloca, i64_ty, "class_bases_ptr")
                    .unwrap();
                let attrs_ptr_bits = self
                    .backend
                    .builder
                    .build_ptr_to_int(attrs_alloca, i64_ty, "class_attrs_ptr")
                    .unwrap();
                let class_def_fn = self.ensure_runtime_i64_fn("molt_guarded_class_def", 8);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        class_def_fn,
                        &[
                            name_bits.into(),
                            bases_ptr_bits.into(),
                            i64_ty.const_int(nbases as u64, false).into(),
                            attrs_ptr_bits.into(),
                            i64_ty.const_int(nattrs as u64, false).into(),
                            i64_ty.const_int(layout_size as u64, true).into(),
                            i64_ty.const_int(layout_version as u64, true).into(),
                            i64_ty.const_int(flags as u64, true).into(),
                        ],
                        "class_def",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "module_new" => {
                let Some(&name_id) = op.operands.first() else {
                    return false;
                };
                let module_new_fn = self.ensure_runtime_i64_fn("molt_module_new", 1);
                let name_bits = self.ensure_i64(self.resolve(name_id));
                let result = self
                    .backend
                    .builder
                    .build_call(module_new_fn, &[name_bits.into()], "module_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "module_cache_get" => {
                let Some(&name_id) = op.operands.first() else {
                    return false;
                };
                let get_fn = self.ensure_runtime_i64_fn("molt_module_cache_get", 1);
                let name_bits = self.ensure_i64(self.resolve(name_id));
                let result = self
                    .backend
                    .builder
                    .build_call(get_fn, &[name_bits.into()], "module_cache_get")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "module_cache_set" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let set_fn = self.ensure_runtime_i64_fn("molt_module_cache_set", 2);
                let name_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let module_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[name_bits.into(), module_bits.into()],
                        "module_cache_set",
                    )
                    .unwrap();
                true
            }
            "module_cache_del" => {
                let Some(&name_id) = op.operands.first() else {
                    return false;
                };
                let del_fn = self.ensure_runtime_i64_fn("molt_module_cache_del", 1);
                let name_bits = self.ensure_i64(self.resolve(name_id));
                let _ = self
                    .backend
                    .builder
                    .build_call(del_fn, &[name_bits.into()], "module_cache_del")
                    .unwrap();
                true
            }
            "module_get_attr" | "module_import_from" => {
                if op.operands.len() != 2 {
                    return false;
                }
                // `from M import name` (module_import_from) uses CPython
                // IMPORT_FROM semantics — ImportError on miss with a sys.modules
                // submodule fallback; plain `M.name` raises AttributeError.
                let runtime_symbol = if kind == "module_import_from" {
                    "molt_module_import_from"
                } else {
                    "molt_module_get_attr"
                };
                let get_fn = self.ensure_runtime_i64_fn(runtime_symbol, 2);
                let module_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let attr_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        get_fn,
                        &[module_bits.into(), attr_bits.into()],
                        "module_get_attr",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "module_get_global" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let get_fn = self.ensure_runtime_i64_fn("molt_module_get_global", 2);
                let module_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let attr_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        get_fn,
                        &[module_bits.into(), attr_bits.into()],
                        "module_get_global",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "module_set_attr" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let set_fn = self.ensure_runtime_i64_fn("molt_module_set_attr", 3);
                let module_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let attr_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let val_bits = self.ensure_i64(self.resolve(op.operands[2]));
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[module_bits.into(), attr_bits.into(), val_bits.into()],
                        "module_set_attr",
                    )
                    .unwrap();
                true
            }
            "module_del_global" | "module_del_global_if_present" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let runtime_name = if kind == "module_del_global_if_present" {
                    "molt_module_del_global_if_present"
                } else {
                    "molt_module_del_global"
                };
                let del_fn = self.ensure_runtime_i64_fn(runtime_name, 2);
                let module_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let attr_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let _ = self
                    .backend
                    .builder
                    .build_call(del_fn, &[module_bits.into(), attr_bits.into()], kind)
                    .unwrap();
                true
            }
            "exception_class" => {
                let Some(&kind_id) = op.operands.first() else {
                    return false;
                };
                let class_fn = self.ensure_runtime_i64_fn("molt_exception_class", 1);
                let kind_bits = self.ensure_i64(self.resolve(kind_id));
                let result = self
                    .backend
                    .builder
                    .build_call(class_fn, &[kind_bits.into()], "exception_class")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_new" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let new_fn = self.ensure_runtime_i64_fn("molt_exception_new", 2);
                let kind_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let args_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[kind_bits.into(), args_bits.into()],
                        "exception_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_new_builtin" => {
                let Some(&args_id) = op.operands.first() else {
                    return false;
                };
                let Some(AttrValue::Int(tag)) = op.attrs.get("value") else {
                    return false;
                };
                let new_fn = self.ensure_runtime_i64_fn("molt_exception_new_builtin", 2);
                let tag_val = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(*tag as u64, false);
                let args_bits = self.ensure_i64(self.resolve(args_id));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[tag_val.into(), args_bits.into()],
                        "exception_new_builtin",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_new_builtin_empty" => {
                let Some(AttrValue::Int(tag)) = op.attrs.get("value") else {
                    return false;
                };
                let new_fn = self.ensure_runtime_i64_fn("molt_exception_new_builtin_empty", 1);
                let tag_val = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(*tag as u64, false);
                let result = self
                    .backend
                    .builder
                    .build_call(new_fn, &[tag_val.into()], "exception_new_builtin_empty")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_new_builtin_one" => {
                let Some(&arg_id) = op.operands.first() else {
                    return false;
                };
                let Some(AttrValue::Int(tag)) = op.attrs.get("value") else {
                    return false;
                };
                let new_fn = self.ensure_runtime_i64_fn("molt_exception_new_builtin_one", 2);
                let tag_val = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(*tag as u64, false);
                let arg_bits = self.ensure_i64(self.resolve(arg_id));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[tag_val.into(), arg_bits.into()],
                        "exception_new_builtin_one",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_push" => {
                let push_fn = self.ensure_runtime_i64_fn("molt_exception_push", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(push_fn, &[], "exception_push")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_stack_enter" => {
                let enter_fn = self.ensure_runtime_i64_fn("molt_exception_stack_enter", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(enter_fn, &[], "exception_stack_enter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_stack_depth" => {
                let depth_fn = self.ensure_runtime_i64_fn("molt_exception_stack_depth", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(depth_fn, &[], "exception_stack_depth")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_stack_set_depth" => {
                let Some(&depth_id) = op.operands.first() else {
                    return false;
                };
                let set_fn = self.ensure_runtime_i64_fn("molt_exception_stack_set_depth", 1);
                let depth_bits = self.ensure_i64(self.resolve(depth_id));
                let result = self
                    .backend
                    .builder
                    .build_call(set_fn, &[depth_bits.into()], "exception_stack_set_depth")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_stack_exit" => {
                let Some(&prev_id) = op.operands.first() else {
                    return false;
                };
                let exit_fn = self.ensure_runtime_i64_fn("molt_exception_stack_exit", 1);
                let prev_bits = self.ensure_i64(self.resolve(prev_id));
                let result = self
                    .backend
                    .builder
                    .build_call(exit_fn, &[prev_bits.into()], "exception_stack_exit")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_pop" => {
                let pop_fn = self.ensure_runtime_i64_fn("molt_exception_pop", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(pop_fn, &[], "exception_pop")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_stack_clear" => {
                let clear_fn = self.ensure_runtime_i64_fn("molt_exception_stack_clear", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(clear_fn, &[], "exception_stack_clear")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_last" => {
                let last_fn = self.ensure_runtime_i64_fn("molt_exception_last", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(last_fn, &[], "exception_last")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_last_pending" | "exception_finally_pending_observer" => {
                let last_fn = self.ensure_runtime_i64_fn("molt_exception_last_pending", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(last_fn, &[], "exception_last_pending")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_active" => {
                let active_fn = self.ensure_runtime_i64_fn("molt_exception_active", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(active_fn, &[], "exception_active")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_current" => {
                let current_fn = self.ensure_runtime_i64_fn("molt_exception_current", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(current_fn, &[], "exception_current")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_enter_handler" => {
                let Some(&captured_id) = op.operands.first() else {
                    return false;
                };
                let enter_fn = self.ensure_runtime_i64_fn("molt_exception_enter_handler", 1);
                let captured_bits = self.ensure_i64(self.resolve(captured_id));
                let result = self
                    .backend
                    .builder
                    .build_call(enter_fn, &[captured_bits.into()], "exception_enter_handler")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_resolve_captured" => {
                let Some(&captured_id) = op.operands.first() else {
                    return false;
                };
                let resolve_fn = self.ensure_runtime_i64_fn("molt_exception_resolve_captured", 1);
                let captured_bits = self.ensure_i64(self.resolve(captured_id));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        resolve_fn,
                        &[captured_bits.into()],
                        "exception_resolve_captured",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_clear" => {
                let clear_fn = self.ensure_runtime_i64_fn("molt_exception_clear", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(clear_fn, &[], "exception_clear")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_set_last" => {
                let Some(&exc_id) = op.operands.first() else {
                    return false;
                };
                let set_fn = self.ensure_runtime_i64_fn("molt_exception_set_last", 1);
                let exc_bits = self.ensure_i64(self.resolve(exc_id));
                let result = self
                    .backend
                    .builder
                    .build_call(set_fn, &[exc_bits.into()], "exception_set_last")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_context_set" => {
                let Some(&exc_id) = op.operands.first() else {
                    return false;
                };
                let set_fn = self.ensure_runtime_i64_fn("molt_exception_context_set", 1);
                let exc_bits = self.ensure_i64(self.resolve(exc_id));
                let result = self
                    .backend
                    .builder
                    .build_call(set_fn, &[exc_bits.into()], "exception_context_set")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "builtin_type" => {
                let Some(&tag_id) = op.operands.first() else {
                    return false;
                };
                let builtin_type_fn = self.ensure_runtime_i64_fn("molt_builtin_type", 1);
                let tag_value = self.resolve(tag_id);
                let tag_ty = self
                    .value_types
                    .get(&tag_id)
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                let tag_bits = self.materialize_dynbox_bits(tag_value, &tag_ty);
                let result = self
                    .backend
                    .builder
                    .build_call(builtin_type_fn, &[tag_bits.into()], "builtin_type")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "class_apply_set_name" => {
                let Some(&class_id) = op.operands.first() else {
                    return false;
                };
                let apply_fn = self.ensure_runtime_i64_fn("molt_class_apply_set_name", 1);
                let class_bits = self.ensure_i64(self.resolve(class_id));
                let result = self
                    .backend
                    .builder
                    .build_call(apply_fn, &[class_bits.into()], "class_apply_set_name")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "class_layout_version" => {
                let Some(&class_id) = op.operands.first() else {
                    return false;
                };
                let version_fn = self.ensure_runtime_i64_fn("molt_class_layout_version", 1);
                let class_bits = self.ensure_i64(self.resolve(class_id));
                let result = self
                    .backend
                    .builder
                    .build_call(version_fn, &[class_bits.into()], "class_layout_version")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "class_set_layout_version" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let set_fn = self.ensure_runtime_i64_fn("molt_class_set_layout_version", 2);
                let class_bits = self.materialize_dynbox_operand(op.operands[0]);
                let version_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[class_bits.into(), version_bits.into()],
                        "class_set_layout_version",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "object_set_class" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
                let class_bits = self.materialize_dynbox_operand(op.operands[1]);
                let set_fn = self.ensure_runtime_i64_fn("molt_object_set_class", 2);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[obj_ptr_bits.into(), class_bits.into()],
                        "object_set_class",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "class_merge_layout" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let merge_fn = self.ensure_runtime_i64_fn("molt_class_merge_layout", 3);
                let class_bits = self.materialize_dynbox_operand(op.operands[0]);
                let offsets_bits = self.materialize_dynbox_operand(op.operands[1]);
                let size_bits = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        merge_fn,
                        &[class_bits.into(), offsets_bits.into(), size_bits.into()],
                        "class_merge_layout",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "str_from_obj" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let str_fn = self.ensure_runtime_i64_fn("molt_str_from_obj", 1);
                let src_bits = self.materialize_dynbox_operand(src_id);
                let result = self
                    .backend
                    .builder
                    .build_call(str_fn, &[src_bits.into()], "str_from_obj")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // -- repr(x): a fresh owned string, NOT operand 0. --
            // Mirrors WASM/Luau/native `repr_from_obj` ? `molt_repr_from_obj`.
            // Without this arm the Copy fell through to the bit-passthrough,
            // silently returning `x` (a wrong-result miscompile) and aliasing it
            // (a drop-insertion double-free). One operand, owned result.
            "repr_from_obj" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let repr_fn = self.ensure_runtime_i64_fn("molt_repr_from_obj", 1);
                let src_bits = self.materialize_dynbox_operand(src_id);
                let result = self
                    .backend
                    .builder
                    .build_call(repr_fn, &[src_bits.into()], "repr_from_obj")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // -- int(x[, base]): a fresh owned int object, NOT operand 0. --
            // `molt_int_from_obj(val, base, has_base)`. The frontend always emits
            // the 3-operand form (base / has_base default to the no-base sentinel).
            "int_from_obj" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let int_fn = self.ensure_runtime_i64_fn("molt_int_from_obj", 3);
                let val = self.materialize_dynbox_operand(op.operands[0]);
                let base = self.materialize_dynbox_operand(op.operands[1]);
                let has_base = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        int_fn,
                        &[val.into(), base.into(), has_base.into()],
                        "int_from_obj",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // -- float(x): a fresh owned float object, NOT operand 0. --
            "float_from_obj" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let float_fn = self.ensure_runtime_i64_fn("molt_float_from_obj", 1);
                let src_bits = self.materialize_dynbox_operand(src_id);
                let result = self
                    .backend
                    .builder
                    .build_call(float_fn, &[src_bits.into()], "float_from_obj")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // -- obj[start:end] (the slice subscript): a fresh owned object, NOT
            //    operand 0. `molt_slice(obj, start, end)`. THIS is the exact
            //    adversarial-review P0 #1 double-free vector — `s[-5:]` fell
            //    through to the passthrough, returned `s`, and was double-freed. --
            // -- format(val, spec) (f-string field / format()): fresh owned str. --
            // `molt_format_builtin(val, spec)`.
            "string_format" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let fmt_fn = self.ensure_runtime_i64_fn("molt_format_builtin", 2);
                let val = self.materialize_dynbox_operand(op.operands[0]);
                let spec = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(fmt_fn, &[val.into(), spec.into()], "string_format")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // NOTE: `contains` (the `x in y` membership test) is ALSO a fresh-value
            // `Copy` kind (classified `FreshValue` in `alias_analysis`), but it is
            // already lowered explicitly further down via `emit_containment`
            // (`molt_contains` + `NotIn` negation). It therefore never reaches the
            // `Copy` passthrough fatal gate, and adding a second `"contains" =>` arm
            // here would be an unreachable duplicate. Left to its established arm.
            // -- ascii(x): fresh owned str. --
            "ascii_from_obj" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let ascii_fn = self.ensure_runtime_i64_fn("molt_ascii_from_obj", 1);
                let src_bits = self.materialize_dynbox_operand(src_id);
                let result = self
                    .backend
                    .builder
                    .build_call(ascii_fn, &[src_bits.into()], "ascii_from_obj")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // -- slice(start, stop, step): a fresh owned slice object. --
            // -- dict.keys()/values()/items(): fresh owned view objects. --
            // -- enumerate(iterable[, start]): a fresh owned enumerate object. --
            // -- dict(x): a fresh owned dict. --
            // -- complex(real[, imag]): a fresh owned complex. --
            "complex_from_obj" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let complex_fn = self.ensure_runtime_i64_fn("molt_complex_from_obj", 3);
                let val = self.materialize_dynbox_operand(op.operands[0]);
                let imag = self.materialize_dynbox_operand(op.operands[1]);
                let has_imag = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        complex_fn,
                        &[val.into(), imag.into(), has_imag.into()],
                        "complex_from_obj",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // -- object(): a fresh owned bare object. No operands. --
            "object_new" => {
                let object_new_fn = self.ensure_runtime_i64_fn("molt_object_new", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(object_new_fn, &[], "object_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "int_from_str_of_obj" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let int_fn = self.ensure_runtime_i64_fn("molt_int_from_str_of_obj", 3);
                let val_bits = self.materialize_dynbox_operand(op.operands[0]);
                let base_bits = self.materialize_dynbox_operand(op.operands[1]);
                let has_base_bits = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        int_fn,
                        &[val_bits.into(), base_bits.into(), has_base_bits.into()],
                        "int_from_str_of_obj",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "ord" => {
                if op.operands.len() != 1 {
                    return false;
                }
                let ord_fn = self.ensure_runtime_i64_fn("molt_ord", 1);
                let val_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(ord_fn, &[val_bits.into()], "ord")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "ord_at" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let ord_fn = self.ensure_runtime_i64_fn("molt_ord_at", 2);
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let index_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(ord_fn, &[obj_bits.into(), index_bits.into()], "ord_at")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "string_join" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let join_fn = self.ensure_runtime_i64_fn("molt_string_join", 2);
                let sep_bits = self.materialize_dynbox_operand(op.operands[0]);
                let items_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        join_fn,
                        &[sep_bits.into(), items_bits.into()],
                        "string_join",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "isinstance" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let isinstance_fn = self.ensure_runtime_i64_fn("molt_isinstance", 2);
                let obj_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let class_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        isinstance_fn,
                        &[obj_bits.into(), class_bits.into()],
                        "isinstance",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "exception_match_builtin" => {
                let Some(&exc_id) = op.operands.first() else {
                    return false;
                };
                let Some(AttrValue::Int(tag)) = op.attrs.get("value") else {
                    return false;
                };
                let match_fn = self.ensure_runtime_i64_fn("molt_exception_match_builtin", 2);
                let exc_bits = self.ensure_i64(self.resolve(exc_id));
                let tag_val = self
                    .backend
                    .context
                    .i64_type()
                    .const_int(*tag as u64, false);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        match_fn,
                        &[exc_bits.into(), tag_val.into()],
                        "exception_match_builtin",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "issubclass" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let issubclass_fn = self.ensure_runtime_i64_fn("molt_issubclass", 2);
                let sub_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let class_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        issubclass_fn,
                        &[sub_bits.into(), class_bits.into()],
                        "issubclass",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "has_attr_name" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let has_attr_fn = self.ensure_runtime_i64_fn("molt_has_attr_name", 2);
                let obj_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let name_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        has_attr_fn,
                        &[obj_bits.into(), name_bits.into()],
                        "has_attr_name",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "type_of" => {
                let Some(&obj_id) = op.operands.first() else {
                    return false;
                };
                let type_of_fn = self.ensure_runtime_i64_fn("molt_type_of", 1);
                let obj_bits = self.ensure_i64(self.resolve(obj_id));
                let result = self
                    .backend
                    .builder
                    .build_call(type_of_fn, &[obj_bits.into()], "type_of")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "missing" => {
                let missing_fn = self.ensure_runtime_i64_fn("molt_missing", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(missing_fn, &[], "missing")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "is_callable" => {
                let Some(&obj_id) = op.operands.first() else {
                    return false;
                };
                let callable_fn = self.ensure_runtime_i64_fn("molt_is_callable", 1);
                let obj_bits = self.ensure_i64(self.resolve(obj_id));
                let result = self
                    .backend
                    .builder
                    .build_call(callable_fn, &[obj_bits.into()], "is_callable")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "get_attr_name_default" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let get_fn = self.ensure_runtime_i64_fn("molt_get_attr_name_default", 3);
                let obj_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let name_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let default_bits = self.ensure_i64(self.resolve(op.operands[2]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        get_fn,
                        &[obj_bits.into(), name_bits.into(), default_bits.into()],
                        "get_attr_name_default",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "context_depth" => {
                let depth_fn = self.ensure_runtime_i64_fn("molt_context_depth", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(depth_fn, &[], "context_depth")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "context_unwind_to" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let unwind_fn = self.ensure_runtime_i64_fn("molt_context_unwind_to", 2);
                let depth_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let exc_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        unwind_fn,
                        &[depth_bits.into(), exc_bits.into()],
                        "context_unwind_to",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dataclass_new" => {
                if op.operands.len() != 4 {
                    return false;
                }
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let field_names_bits = self.materialize_dynbox_operand(op.operands[1]);
                let values_bits = self.materialize_dynbox_operand(op.operands[2]);
                let flags_bits = self.materialize_dynbox_operand(op.operands[3]);
                let ctor_fn = self.ensure_runtime_i64_fn("molt_dataclass_new", 4);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        ctor_fn,
                        &[
                            name_bits.into(),
                            field_names_bits.into(),
                            values_bits.into(),
                            flags_bits.into(),
                        ],
                        "dataclass_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "dataclass_new_values" => {
                if op.operands.len() < 3 {
                    return false;
                }
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let field_names_bits = self.materialize_dynbox_operand(op.operands[1]);
                let flags_bits = self.materialize_dynbox_operand(op.operands[2]);
                let value_ids = &op.operands[3..];
                let values_ptr_bits = if value_ids.is_empty() {
                    i64_ty.const_zero()
                } else {
                    let values_alloca = self
                        .backend
                        .builder
                        .build_array_alloca(
                            i64_ty,
                            i64_ty.const_int(value_ids.len() as u64, false),
                            "dataclass_values",
                        )
                        .unwrap();
                    for (idx, &value_id) in value_ids.iter().enumerate() {
                        let value_bits = self.materialize_dynbox_operand(value_id);
                        let elem_ptr = unsafe {
                            self.backend
                                .builder
                                .build_gep(
                                    i64_ty,
                                    values_alloca,
                                    &[i64_ty.const_int(idx as u64, false)],
                                    &format!("dataclass_value_ptr_{idx}"),
                                )
                                .unwrap()
                        };
                        self.backend
                            .builder
                            .build_store(elem_ptr, value_bits)
                            .unwrap();
                    }
                    self.backend
                        .builder
                        .build_ptr_to_int(values_alloca, i64_ty, "dataclass_values_ptr")
                        .unwrap()
                };
                let ctor_fn = self.ensure_runtime_i64_fn("molt_dataclass_new_from_values", 5);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        ctor_fn,
                        &[
                            name_bits.into(),
                            field_names_bits.into(),
                            values_ptr_bits.into(),
                            i64_ty.const_int(value_ids.len() as u64, false).into(),
                            flags_bits.into(),
                        ],
                        "dataclass_new_values",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            // -- Preserved value-producing / side-effecting ops whose runtime
            //    symbol name DIFFERS from `molt_<kind>` (so the generic
            //    `try_lower_preserved_runtime_call` fallback declines them), or
            //    which are RESULT-LESS side effects the runtime-call fallback
            //    refuses on principle. Each is the byte-for-byte LLVM analogue
            //    of the native (`function_compiler{,/fc/*}.rs`) handler, with the
            //    SAME runtime symbol and operand convention. Before these arms
            //    landed, every one of these kinds fell to the `Copy`
            //    passthrough: 0-operand singletons (`...`, `NotImplemented`)
            //    became `None`; `abs(x)` returned `x`; generator `throw`/`close`,
            //    the `__cause__` chain link, special-attr loads, the RC alias
            //    ops, and the type/layout guards were all silently DROPPED. --

            // `abs(x)` — boxed builtin (the native int-lane branchless fast path
            // does not apply on the TIR/LLVM lane, which has no raw-int primary
            // vars; the boxed path is correct and overflow-safe for BigInt).
            // Symbol is `molt_abs_builtin`, NOT `molt_abs`.
            "abs" => {
                let Some(&x_id) = op.operands.first() else {
                    return false;
                };
                let abs_fn = self.ensure_runtime_i64_fn("molt_abs_builtin", 1);
                let x_bits = self.materialize_dynbox_operand(x_id);
                let result = self
                    .backend
                    .builder
                    .build_call(abs_fn, &[x_bits.into()], "abs")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // `...` literal ? the Ellipsis singleton. Symbol `molt_ellipsis`,
            // NOT `molt_const_ellipsis`. 0 operands.
            "const_ellipsis" => {
                let ell_fn = self.ensure_runtime_i64_fn("molt_ellipsis", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(ell_fn, &[], "const_ellipsis")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // `NotImplemented` singleton (e.g. a `__eq__` returning it). Symbol
            // `molt_not_implemented`, NOT `molt_const_not_implemented`.
            "const_not_implemented" => {
                let ni_fn = self.ensure_runtime_i64_fn("molt_not_implemented", 0);
                let result = self
                    .backend
                    .builder
                    .build_call(ni_fn, &[], "const_not_implemented")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // `gen.throw(exc)` ? `molt_generator_throw(gen, val)` (operands
            // [gen, val]). Symbol differs from `molt_gen_throw`.
            "gen_throw" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let throw_fn = self.ensure_runtime_i64_fn("molt_generator_throw", 2);
                let gen_bits = self.materialize_dynbox_operand(op.operands[0]);
                let val_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(throw_fn, &[gen_bits.into(), val_bits.into()], "gen_throw")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // `gen.close()` ? `molt_generator_close(gen)` (operand [gen]).
            // Symbol differs from `molt_gen_close`.
            "gen_close" => {
                let Some(&gen_id) = op.operands.first() else {
                    return false;
                };
                let close_fn = self.ensure_runtime_i64_fn("molt_generator_close", 1);
                let gen_bits = self.materialize_dynbox_operand(gen_id);
                let result = self
                    .backend
                    .builder
                    .build_call(close_fn, &[gen_bits.into()], "gen_close")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // `raise X from Y` cause link ? `molt_exception_set_cause(exc,
            // cause)`. Symbol matches `molt_<kind>`, but the op is frequently
            // RESULT-LESS (a pure side effect) so the runtime-call fallback
            // declines it (its `op.results.first()` early-return). Emit the call
            // unconditionally; bind the result only when present. Mirrors the
            // existing `exception_set_last` arm.
            "exception_set_cause" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let set_fn = self.ensure_runtime_i64_fn("molt_exception_set_cause", 2);
                let exc_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let cause_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[exc_bits.into(), cause_bits.into()],
                        "exception_set_cause",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // Special-attribute load (`__class__`, `__name__`, …) ?
            // `molt_get_attr_special(obj, name_ptr, name_len)`. The attribute
            // name is a compile-time string carried in `s_value`, materialized as
            // a private constant (the label-carrying convention, identical to the
            // native handler and the `call_method_ic` arm above).
            //
            // OWNERSHIP: `molt_get_attr_special` returns a BORROWED reference
            // (the value comes from `class_attr_lookup` / a descriptor / a slot
            // — not a fresh allocation). The native handler
            // (`fc/attrs.rs::get_attr_special_obj`) therefore inc_refs the result
            // via `emit_maybe_ref_adjust_v2(res, molt_inc_ref_obj)` to take owned
            // ownership; the existing LLVM `get_attr_generic_obj` arm
            // (`molt_get_attr_object_ic`) does the same. We MUST mirror that here:
            // binding the borrowed result without the inc_ref under-counts it and
            // risks a premature free / use-after-free of the attribute object.
            "get_attr_special_obj" => {
                let Some(&obj_id) = op.operands.first() else {
                    return false;
                };
                let Some(attr_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.clone()),
                    _ => None,
                }) else {
                    return false;
                };
                let obj_bits = self.materialize_dynbox_operand(obj_id);
                let (name_ptr_bits, name_len_bits) = self.raw_string_const_ptr_len(&attr_name);
                let get_fn = self.ensure_runtime_i64_fn("molt_get_attr_special", 3);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        get_fn,
                        &[obj_bits.into(), name_ptr_bits.into(), name_len_bits.into()],
                        "get_attr_special_obj",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                // Take owned ownership of the borrowed attribute result (mirrors
                // the native get-attr ref-adjust). `molt_inc_ref_obj` is a no-op
                // for NaN-boxed immediates, so this is safe for any tag.
                let inc_fn = self.ensure_runtime_import(MOLT_INC_REF_OBJ);
                self.backend
                    .builder
                    .build_call(
                        inc_fn,
                        &[self.ensure_i64(result).into()],
                        "get_attr_special_inc_ref",
                    )
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // RC-alias ops: `borrow`/retained aliases == `inc_ref` then ALIAS the
            // value through (result == source operand). The native handlers
            // (`function_compiler.rs` `inc_ref|borrow` / retained aliases) emit
            // `molt_inc_ref_obj(src)` and `def_var(out, src)` — a plain Copy
            // passthrough would skip the inc_ref (a refcount LEAK). `release` is
            // the dual and is handled in its OWN arm below because its result
            // convention differs (it does NOT alias the source — see there).
            "borrow" | "identity_alias" | "binding_alias" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let src_val = self.resolve(src_id);
                let src_bits = self.ensure_i64(src_val);
                let inc_fn = self.ensure_runtime_import(MOLT_INC_REF_OBJ);
                self.backend
                    .builder
                    .build_call(inc_fn, &[src_bits.into()], "")
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    let ty = self
                        .value_types
                        .get(&src_id)
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    self.values.insert(result_id, src_val);
                    self.value_types.insert(result_id, ty);
                }
                true
            }
            // `release` == `dec_ref` the source. CRITICAL: unlike `borrow`, the
            // result must NOT alias the source — after `molt_dec_ref_obj` the
            // source may be freed, so aliasing+using it is a use-after-free. The
            // native handler (`function_compiler.rs` `dec_ref|release`) dec_refs
            // the source and, when the op carries an out var, binds it to NONE
            // (`def_var_named(out, box_none())`), never to the released source.
            // We mirror that: emit the dec_ref, then bind any result to None.
            "release" => {
                let Some(&src_id) = op.operands.first() else {
                    return false;
                };
                let src_val = self.resolve(src_id);
                let src_bits = self.ensure_i64(src_val);
                let dec_fn = self.ensure_runtime_import(MOLT_DEC_REF_OBJ);
                self.backend
                    .builder
                    .build_call(dec_fn, &[src_bits.into()], "")
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    let none_val: BasicValueEnum<'ctx> = i64_ty
                        .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                        .into();
                    self.values.insert(result_id, none_val);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            // Generator-frame locals registration (introspection support for
            // `gi_frame.f_locals` / `frame_locals_set`). Result-less side effect:
            // `molt_gen_locals_register(func_addr, names_tuple, offsets_tuple)`
            // where `func_addr` is the ADDRESS of the generator function named in
            // `s_value` (cast to i64, exactly like `func_new`), and the two
            // operands are the boxed names/offsets tuples. Dropping it (the old
            // `Copy` passthrough) silently diverges generator-frame introspection
            // from CPython. Mirrors the native handler
            // (function_compiler.rs `gen_locals_register`).
            "gen_locals_register" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.clone()),
                    _ => None,
                }) else {
                    return false;
                };
                let arity = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => usize::try_from(*v).ok(),
                        _ => None,
                    })
                    .unwrap_or(0);
                let func = self.ensure_function_symbol(&func_name, arity, false);
                let func_addr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "gen_locals_func_ptr",
                    )
                    .unwrap();
                let names_bits = self.materialize_dynbox_operand(op.operands[0]);
                let offsets_bits = self.materialize_dynbox_operand(op.operands[1]);
                let reg_fn = self.ensure_runtime_i64_fn("molt_gen_locals_register", 3);
                self.backend
                    .builder
                    .build_call(
                        reg_fn,
                        &[func_addr.into(), names_bits.into(), offsets_bits.into()],
                        "gen_locals_register",
                    )
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    let none_val: BasicValueEnum<'ctx> = i64_ty
                        .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                        .into();
                    self.values.insert(result_id, none_val);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            // Type/tag guard: a runtime CHECK that raises `TypeError` on
            // mismatch; the return value is discarded (the op is result-less on
            // native). `molt_guard_type(val, expected)`. A passthrough here would
            // SILENTLY ELIDE the guard — the program would not raise where
            // CPython does. (`guard_type` is the canonical kind and IS mapped to
            // a dedicated TIR `OpCode::TypeGuard`; only the `guard_tag` alias
            // reaches here as a preserved `Copy`, but we keep `guard_type` in the
            // arm for completeness/idempotence.)
            "guard_type" | "guard_tag" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let guard_fn = self.ensure_runtime_i64_fn("molt_guard_type", 2);
                let val_bits = self.materialize_dynbox_operand(op.operands[0]);
                let expected_bits = self.materialize_dynbox_operand(op.operands[1]);
                self.backend
                    .builder
                    .build_call(
                        guard_fn,
                        &[val_bits.into(), expected_bits.into()],
                        "guard_type",
                    )
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    // Guard return is the conventional sentinel; rebind only if a
                    // result was requested (native discards it).
                    let none_val: BasicValueEnum<'ctx> = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                        .into();
                    self.values.insert(result_id, none_val);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            // Layout / dict-shape guard (polymorphic-inline-cache fast path):
            // `molt_guard_layout_ptr(obj_ptr, class, expected_version)`. Both
            // `guard_layout` and `guard_dict_shape` share this single runtime
            // entry (the native handler groups them identically). Three points
            // make the generic `molt_<kind>` fallback INCAPABLE of lowering these
            // correctly — hence the dedicated arm:
            //   1. The runtime symbol is `molt_guard_layout_ptr`, not
            //      `molt_guard_layout` / `molt_guard_dict_shape`.
            //   2. The first argument is the RAW UNBOXED heap pointer of the
            //      object (`unbox_ptr_bits`), not the NaN-boxed value — mirroring
            //      the native `unbox_ptr_value(*obj)` before the call.
            //   3. The op carries a result on the IC fast path, but the guard
            //      VALUE is conventionally discarded (it raises on mismatch).
            // A `Copy` passthrough here would silently ELIDE the shape check, so
            // a stale-layout object would skip the deopt/guard and the program
            // would not raise / would read the wrong slot where CPython is
            // type-safe. operands = [obj, class, expected_version].
            "guard_layout" | "guard_dict_shape" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let obj_ptr = self.unbox_ptr_bits(obj_bits);
                let class_bits = self.materialize_dynbox_operand(op.operands[1]);
                let version_bits = self.materialize_dynbox_operand(op.operands[2]);
                let guard_fn = self.ensure_runtime_i64_fn("molt_guard_layout_ptr", 3);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        guard_fn,
                        &[obj_ptr.into(), class_bits.into(), version_bits.into()],
                        "guard_layout",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "guarded_field_init" => {
                if op.operands.len() != 4 {
                    return false;
                }
                let Some(attr_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.clone()),
                    _ => None,
                }) else {
                    return false;
                };
                let offset = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
                let class_bits = self.materialize_dynbox_operand(op.operands[1]);
                let expected_version = self.materialize_dynbox_operand(op.operands[2]);
                let val_bits = self.materialize_dynbox_operand(op.operands[3]);
                let (attr_ptr_bits, attr_len_bits) = self.raw_string_const_ptr_len(&attr_name);
                let init_fn = self.ensure_runtime_i64_fn("molt_guarded_field_init_ptr", 7);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        init_fn,
                        &[
                            obj_ptr_bits.into(),
                            class_bits.into(),
                            expected_version.into(),
                            i64_ty.const_int(offset as u64, true).into(),
                            val_bits.into(),
                            attr_ptr_bits.into(),
                            attr_len_bits.into(),
                        ],
                        "guarded_field_init",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            // Structured-data scalar parse (`json.loads`/`msgpack`/`cbor` on a
            // single scalar): `molt_<fmt>_parse_scalar_obj(value)`. The native
            // handler (`fc::parse_ops::handle_parse_op`) has a raw-pointer FAST
            // path (it reads `{arg}_ptr`/`{arg}_len` companion vars into a stack
            // out-param via `molt_<fmt>_parse_scalar`) AND this boxed SLOW path
            // for the general case. The fast path is a pure perf optimization
            // keyed on a native-only raw-string-pointer var convention the
            // TIR/LLVM lane does not carry; the slow `*_scalar_obj(value)` call is
            // the SEMANTICALLY COMPLETE lowering native falls back to whenever the
            // companion vars are absent (its `else` branch), so the LLVM lane uses
            // it unconditionally — same result, no fast-path reboxing avoidance.
            // The generic `molt_<kind>` fallback cannot claim these: the symbol is
            // `molt_<fmt>_parse_scalar_obj`, not `molt_<fmt>_parse`. operands =
            // [value]. A `Copy` passthrough would return the unparsed input.
            "json_parse" | "msgpack_parse" | "cbor_parse" => {
                let Some(&val_id) = op.operands.first() else {
                    return false;
                };
                let symbol = match kind {
                    "json_parse" => "molt_json_parse_scalar_obj",
                    "msgpack_parse" => "molt_msgpack_parse_scalar_obj",
                    "cbor_parse" => "molt_cbor_parse_scalar_obj",
                    _ => unreachable!("outer match restricts kind to the three parse ops"),
                };
                let val_bits = self.materialize_dynbox_operand(val_id);
                let parse_fn = self.ensure_runtime_i64_fn(symbol, 1);
                let result = self
                    .backend
                    .builder
                    .build_call(parse_fn, &[val_bits.into()], "parse_scalar")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }

            // -- Arithmetic / comparison / bitwise carried as preserved `Copy`
            //    ops --
            //
            // The SimpleIR?TIR lift (`kind_to_opcode`) maps some operator kinds
            // the frontend emits — `floordiv`, `invert`, `contains`, the
            // `inplace_bit_*` family, `matmul`, `pow_mod` — to `OpCode::Copy`
            // with `_original_kind` preserved, rather than to their dedicated
            // opcodes. The native/Cranelift and WASM lanes consume SimpleIR
            // directly (where these are real op kinds) and are unaffected, but
            // the LLVM lane lowers the TIR, where these arrive as `Copy`. Without
            // this arm the generic `Copy` handler falls through to "pass through
            // operand 0", silently replacing e.g. `a // b` with `a` (and dropping
            // any exception the operator would raise) — a silent miscompile of
            // every such operator on the LLVM lane.
            //
            // Each kind is lowered with the SAME emit helper its dedicated opcode
            // uses (the helpers take the operator name as a parameter and do not
            // read `op.opcode`; `emit_containment` checks only for `NotIn`, so the
            // `Copy`-carried `in`/`contains` correctly take the non-negated path).
            // `matmul`/`pow_mod` have no dedicated opcode or arith-specialized
            // path, so they lower to their boxed runtime calls (mirroring WASM).
            "floordiv" => {
                self.emit_binary_arith(op, "floordiv");
                true
            }
            "invert" => {
                self.emit_unary(op, "invert");
                true
            }
            "contains" => {
                // `x in y`. `emit_containment` negates only for `OpCode::NotIn`;
                // a `Copy`-carried `contains` is the affirmative membership test.
                self.emit_containment(op);
                true
            }
            "inplace_bit_and" => {
                self.emit_bitwise(op, "bit_and");
                true
            }
            "inplace_bit_or" => {
                self.emit_bitwise(op, "bit_or");
                true
            }
            "inplace_bit_xor" => {
                self.emit_bitwise(op, "bit_xor");
                true
            }
            // In-place augmented arithmetic for `//=`, `%=`, `**=`, `<<=`, `>>=`.
            // These ride `Copy{_original_kind}` (no first-class opcode, mirroring
            // `floordiv`/`inplace_bit_*`). We lower them with the SAME fast int/
            // float lane emitter as their binary opcode (the static int/float path
            // is byte-identical — builtin numerics have no in-place dunder), and
            // `emit_binary_arith`/`emit_bitwise` detect the `inplace_` prefix on
            // `_original_kind` to route the BOXED slow path to
            // `molt_inplace_<op>` (which tries `__i<op>__` first). `@=`/
            // `inplace_matmul` has no arith-specialized path and falls through to
            // the generic runtime-call fallback below, which emits
            // `molt_inplace_matmul`.
            "inplace_div" => {
                self.emit_binary_arith(op, "div");
                true
            }
            "inplace_floordiv" => {
                self.emit_binary_arith(op, "floordiv");
                true
            }
            "inplace_mod" => {
                self.emit_binary_arith(op, "mod");
                true
            }
            "inplace_pow" => {
                self.emit_binary_arith(op, "pow");
                true
            }
            "inplace_lshift" => {
                self.emit_bitwise(op, "lshift");
                true
            }
            "inplace_rshift" => {
                self.emit_bitwise(op, "rshift");
                true
            }
            _ => unreachable!("llvm preserved direct-op family routed unsupported kind `{kind}`"),
        }
    }
}
