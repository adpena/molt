use super::*;

pub(super) fn is_preserved_callable_kind(kind: &str) -> bool {
    matches!(
        kind,
        "builtin_func"
            | "func_new"
            | "func_new_closure"
            | "code_new"
            | "code_slot_set"
            | "code_slots_init"
            | "classmethod_new"
            | "staticmethod_new"
            | "property_new"
            | "trace_enter_slot"
            | "trace_exit"
            | "frame_locals_set"
            | "line"
            | "fn_ptr_code_set"
            | "callargs_new"
            | "callargs_push_pos"
            | "callargs_push_kw"
            | "callargs_expand_star"
            | "callargs_expand_kwstar"
    )
}

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn lower_preserved_callable_op(&mut self, op: &TirOp, kind: &str) -> bool {
        let i64_ty = self.backend.context.i64_type();
        match kind {
            "builtin_func" => {
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
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
                let func = self.ensure_function_symbol(func_name, arity, false);
                let fn_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "builtin_func_ptr",
                    )
                    .unwrap();
                let trampoline = self.ensure_plain_trampoline(func_name, arity, false);
                let tramp_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        trampoline.as_global_value().as_pointer_value(),
                        i64_ty,
                        "builtin_trampoline_ptr",
                    )
                    .unwrap();
                let name_bits = self.intern_string_const(func_name).into_int_value();
                let new_fn = self.ensure_runtime_i64_fn("molt_func_new_builtin_named", 4);
                let func_bits = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[
                            name_bits.into(),
                            fn_ptr.into(),
                            tramp_ptr.into(),
                            i64_ty.const_int(arity as u64, false).into(),
                        ],
                        "builtin_func_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, func_bits);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "func_new" => {
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
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
                let func = self.ensure_function_symbol(func_name, arity, false);
                let fn_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "func_ptr",
                    )
                    .unwrap();
                let trampoline = self.ensure_plain_trampoline(func_name, arity, false);
                let tramp_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        trampoline.as_global_value().as_pointer_value(),
                        i64_ty,
                        "func_trampoline_ptr",
                    )
                    .unwrap();
                let new_fn = self.ensure_runtime_i64_fn("molt_func_new", 3);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[
                            fn_ptr.into(),
                            tramp_ptr.into(),
                            i64_ty.const_int(arity as u64, false).into(),
                        ],
                        "func_new",
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
            "func_new_closure" => {
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
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
                let Some(&closure_id) = op.operands.first() else {
                    return false;
                };
                let closure_bits = self.ensure_i64(self.resolve(closure_id));
                let func = self.ensure_function_symbol(func_name, arity, true);
                let fn_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "closure_func_ptr",
                    )
                    .unwrap();
                let trampoline = self.ensure_plain_trampoline(func_name, arity, true);
                let tramp_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        trampoline.as_global_value().as_pointer_value(),
                        i64_ty,
                        "closure_trampoline_ptr",
                    )
                    .unwrap();
                let new_fn = self.ensure_runtime_i64_fn("molt_func_new_closure", 4);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[
                            fn_ptr.into(),
                            tramp_ptr.into(),
                            i64_ty.const_int(arity as u64, false).into(),
                            closure_bits.into(),
                        ],
                        "func_new_closure",
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
            "code_new" => {
                if op.operands.len() != 9 {
                    return false;
                }
                let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = op
                    .operands
                    .iter()
                    .map(|&id| self.materialize_dynbox_operand(id).into())
                    .collect();
                let code_new_fn = self.ensure_runtime_i64_fn("molt_code_new", 9);
                let result = self
                    .backend
                    .builder
                    .build_call(code_new_fn, &args, "code_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "code_slot_set" => {
                let code_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let Some(&code_bits_id) = op.operands.first() else {
                    return false;
                };
                let code_bits = self.ensure_i64(self.resolve(code_bits_id));
                let slot_set_fn = self.ensure_runtime_i64_fn("molt_code_slot_set", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        slot_set_fn,
                        &[
                            i64_ty.const_int(code_id as u64, true).into(),
                            code_bits.into(),
                        ],
                        "code_slot_set",
                    )
                    .unwrap();
                true
            }
            "code_slots_init" => {
                let count = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let init_fn = self.ensure_runtime_i64_fn("molt_code_slots_init", 1);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        init_fn,
                        &[i64_ty.const_int(count as u64, true).into()],
                        "code_slots_init",
                    )
                    .unwrap();
                true
            }
            "classmethod_new" => {
                let Some(&func_id) = op.operands.first() else {
                    return false;
                };
                let func_bits = self.ensure_i64(self.resolve(func_id));
                let classmethod_fn = self.ensure_runtime_i64_fn("molt_classmethod_new", 1);
                let result = self
                    .backend
                    .builder
                    .build_call(classmethod_fn, &[func_bits.into()], "classmethod_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "staticmethod_new" => {
                let Some(&func_id) = op.operands.first() else {
                    return false;
                };
                let func_bits = self.ensure_i64(self.resolve(func_id));
                let staticmethod_fn = self.ensure_runtime_i64_fn("molt_staticmethod_new", 1);
                let result = self
                    .backend
                    .builder
                    .build_call(staticmethod_fn, &[func_bits.into()], "staticmethod_new")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                true
            }
            "property_new" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let getter_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let setter_bits = self.ensure_i64(self.resolve(op.operands[1]));
                let deleter_bits = self.ensure_i64(self.resolve(op.operands[2]));
                let property_fn = self.ensure_runtime_i64_fn("molt_property_new", 3);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        property_fn,
                        &[getter_bits.into(), setter_bits.into(), deleter_bits.into()],
                        "property_new",
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
            "trace_enter_slot" => {
                let code_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let enter_fn = self.ensure_runtime_i64_fn("molt_trace_enter_slot", 1);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        enter_fn,
                        &[i64_ty.const_int(code_id as u64, true).into()],
                        "trace_enter_slot",
                    )
                    .unwrap();
                true
            }
            "trace_exit" => {
                let exit_fn = self.ensure_runtime_i64_fn("molt_trace_exit", 0);
                let _ = self
                    .backend
                    .builder
                    .build_call(exit_fn, &[], "trace_exit")
                    .unwrap();
                true
            }
            "frame_locals_set" => {
                let Some(&dict_id) = op.operands.first() else {
                    return false;
                };
                let frame_locals_fn = self.ensure_runtime_i64_fn("molt_frame_locals_set", 1);
                let dict_bits = self.materialize_dynbox_operand(dict_id);
                let _ = self
                    .backend
                    .builder
                    .build_call(frame_locals_fn, &[dict_bits.into()], "frame_locals_set")
                    .unwrap();
                true
            }
            "line" => {
                let line = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let line_fn = self.ensure_runtime_i64_fn("molt_trace_set_line", 1);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        line_fn,
                        &[i64_ty.const_int(line as u64, true).into()],
                        "trace_set_line",
                    )
                    .unwrap();
                true
            }
            "fn_ptr_code_set" => {
                let Some(func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
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
                let Some(&code_bits_id) = op.operands.first() else {
                    return false;
                };
                let code_bits = self.ensure_i64(self.resolve(code_bits_id));
                let func = self.ensure_function_symbol(func_name, arity, false);
                let fn_ptr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        func.as_global_value().as_pointer_value(),
                        i64_ty,
                        "fn_ptr_code",
                    )
                    .unwrap();
                let set_fn = self.ensure_runtime_i64_fn("molt_fn_ptr_code_set", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[fn_ptr.into(), code_bits.into()],
                        "fn_ptr_code_set",
                    )
                    .unwrap();
                true
            }
            "callargs_new" => {
                let new_fn = self.ensure_runtime_i64_fn("molt_callargs_new", 2);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[i64_ty.const_zero().into(), i64_ty.const_zero().into()],
                        "callargs_new",
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
            "callargs_push_pos" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let push_fn = self.ensure_runtime_i64_fn("molt_callargs_push_pos", 2);
                let builder_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let val_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        push_fn,
                        &[builder_bits.into(), val_bits.into()],
                        "callargs_push_pos",
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
            "callargs_push_kw" => {
                if op.operands.len() != 3 {
                    return false;
                }
                let push_fn = self.ensure_runtime_i64_fn("molt_callargs_push_kw", 3);
                let builder_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let name_bits = self.materialize_dynbox_operand(op.operands[1]);
                let val_bits = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        push_fn,
                        &[builder_bits.into(), name_bits.into(), val_bits.into()],
                        "callargs_push_kw",
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
            "callargs_expand_star" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let expand_fn = self.ensure_runtime_i64_fn("molt_callargs_expand_star", 2);
                let builder_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let iterable_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        expand_fn,
                        &[builder_bits.into(), iterable_bits.into()],
                        "callargs_expand_star",
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
            "callargs_expand_kwstar" => {
                if op.operands.len() != 2 {
                    return false;
                }
                let expand_fn = self.ensure_runtime_i64_fn("molt_callargs_expand_kwstar", 2);
                let builder_bits = self.ensure_i64(self.resolve(op.operands[0]));
                let mapping_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        expand_fn,
                        &[builder_bits.into(), mapping_bits.into()],
                        "callargs_expand_kwstar",
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
            _ => unreachable!("preserved callable kind predicate accepted unknown kind {kind}"),
        }
    }
}
