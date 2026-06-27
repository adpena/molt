use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn emit_load_attr(&mut self, op: &TirOp) {
        let result_id = op.results[0];
        let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
            AttrValue::Str(s) => Some(s.as_str()),
            _ => None,
        });
        if matches!(original_kind, Some("get_attr_name")) && op.operands.len() >= 2 {
            let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
            let name_bits = self.materialize_dynbox_operand(op.operands[1]);
            let get_fn = self.ensure_runtime_i64_fn("molt_get_attr_name", 2);
            let val = self
                .backend
                .builder
                .build_call(
                    get_fn,
                    &[obj_bits.into(), name_bits.into()],
                    "get_attr_name_dyn",
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            self.values.insert(result_id, val);
            self.value_types.insert(result_id, TirType::DynBox);
            return;
        }
        if matches!(original_kind, Some("load")) && !op.operands.is_empty() {
            let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
            let offset = op
                .attrs
                .get("value")
                .and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                })
                .unwrap_or(0);
            let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
            // Inline the field load: convert ptr to pointer type,
            // GEP by byte offset, load i64, then inc_ref.
            // This eliminates the runtime call (GIL + debug checks).
            let i64_ty = self.backend.context.i64_type();
            let i8_ty = self.backend.context.i8_type();
            let ptr_ty = self
                .backend
                .context
                .ptr_type(inkwell::AddressSpace::default());
            let raw_ptr = self
                .backend
                .builder
                .build_int_to_ptr(obj_ptr_bits, ptr_ty, "obj_ptr")
                .unwrap();
            let offset_val = i64_ty.const_int(offset as u64, true);
            let field_ptr = unsafe {
                self.backend
                    .builder
                    .build_in_bounds_gep(i8_ty, raw_ptr, &[offset_val], "field_ptr")
                    .unwrap()
            };
            let val = self
                .backend
                .builder
                .build_load(i64_ty, field_ptr, "field_val")
                .unwrap();
            // inc_ref the loaded value (may be a heap pointer).
            let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
            self.backend
                .builder
                .build_call(inc_fn, &[val.into()], "field_load_inc_ref")
                .unwrap();
            self.values.insert(result_id, val);
            self.value_types.insert(result_id, TirType::DynBox);
            return;
        }
        if matches!(original_kind, Some("guarded_field_get")) && op.operands.len() >= 3 {
            let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
            let class_bits = self.materialize_dynbox_operand(op.operands[1]);
            let expected_version = self.materialize_dynbox_operand(op.operands[2]);
            let attr_name = op
                .attrs
                .get("name")
                .and_then(|v| {
                    if let AttrValue::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("<unknown>");
            let offset = op
                .attrs
                .get("value")
                .and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                })
                .unwrap_or(0);
            let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
            let (attr_ptr_bits, attr_len_bits) = self.raw_string_const_ptr_len(attr_name);
            let get_fn = self.ensure_runtime_i64_fn("molt_guarded_field_get_ptr", 6);
            let val = self
                .backend
                .builder
                .build_call(
                    get_fn,
                    &[
                        obj_ptr_bits.into(),
                        class_bits.into(),
                        expected_version.into(),
                        self.backend
                            .context
                            .i64_type()
                            .const_int(offset as u64, true)
                            .into(),
                        attr_ptr_bits.into(),
                        attr_len_bits.into(),
                    ],
                    "guarded_field_get",
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            self.values.insert(result_id, val);
            self.value_types.insert(result_id, TirType::DynBox);
            return;
        }
        let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
        // Attribute name is stored in attrs["name"], not as a second operand.
        let attr_name = op
            .attrs
            .get("name")
            .and_then(|v| {
                if let AttrValue::Str(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("<unknown>");
        let runtime_name = if matches!(original_kind, Some("get_attr_generic_obj")) {
            "molt_get_attr_object_ic"
        } else {
            "molt_get_attr_name"
        };
        let get_fn = if runtime_name == "molt_get_attr_object_ic" {
            self.ensure_runtime_i64_fn(runtime_name, 4)
        } else {
            self.ensure_runtime_i64_fn(runtime_name, 2)
        };
        let name = self.intern_string_const(attr_name);
        let name_bits = self.ensure_i64(name);
        let site_bits = self.next_call_site_bits("get_attr_generic_obj");
        let (attr_ptr_bits, attr_len_bits) = self.raw_string_const_ptr_len(attr_name);
        let call_args_generic = [
            obj_bits.into(),
            attr_ptr_bits.into(),
            attr_len_bits.into(),
            site_bits.into(),
        ];
        let call_args_name = [obj_bits.into(), name_bits.into()];
        let val = self
            .backend
            .builder
            .build_call(
                get_fn,
                if runtime_name == "molt_get_attr_object_ic" {
                    &call_args_generic
                } else {
                    &call_args_name
                },
                runtime_name,
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if runtime_name == "molt_get_attr_object_ic" {
            let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
            let _ = self
                .backend
                .builder
                .build_call(
                    inc_fn,
                    &[val.into_int_value().into()],
                    "get_attr_object_ic_inc_ref",
                )
                .unwrap();
        }
        self.values.insert(result_id, val);
        self.value_types.insert(result_id, TirType::DynBox);
    }

    pub(super) fn emit_store_attr(&mut self, op: &TirOp) {
        let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
            AttrValue::Str(s) => Some(s.as_str()),
            _ => None,
        });
        if matches!(original_kind, Some("set_attr_name")) && op.operands.len() >= 3 {
            let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
            let name_bits = self.materialize_dynbox_operand(op.operands[1]);
            let val_bits = self.materialize_dynbox_operand(op.operands[2]);
            let set_fn = self.ensure_runtime_i64_fn("molt_set_attr_name", 3);
            let result = self
                .backend
                .builder
                .build_call(
                    set_fn,
                    &[obj_bits.into(), name_bits.into(), val_bits.into()],
                    "set_attr_name_dyn",
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            if !op.results.is_empty() {
                self.values.insert(op.results[0], result);
                self.value_types.insert(op.results[0], TirType::DynBox);
            }
            return;
        }
        if matches!(original_kind, Some("store_init")) && op.operands.len() >= 2 {
            let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
            let val_bits = self.materialize_dynbox_operand(op.operands[1]);
            let offset = op
                .attrs
                .get("value")
                .and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                })
                .unwrap_or(0);
            let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
            // Inline store_init: direct store for immediate values,
            // runtime call only for heap pointers (need inc_ref + mark_has_ptrs).
            let i64_ty = self.backend.context.i64_type();
            let i8_ty = self.backend.context.i8_type();
            let ptr_ty = self
                .backend
                .context
                .ptr_type(inkwell::AddressSpace::default());
            // Check if val is a heap pointer: (val & TAG_MASK) == TAG_PTR
            let tag_mask = i64_ty.const_int(nanbox::QNAN | 0x0007_0000_0000_0000, false);
            let tag_bits = self
                .backend
                .builder
                .build_and(val_bits, tag_mask, "init_tag")
                .unwrap();
            let ptr_tag = i64_ty.const_int(nanbox::QNAN | 0x0004_0000_0000_0000, false);
            let is_ptr = self
                .backend
                .builder
                .build_int_compare(inkwell::IntPredicate::EQ, tag_bits, ptr_tag, "is_ptr")
                .unwrap();
            let current_fn = self.llvm_fn;
            let fast_bb = self
                .backend
                .context
                .append_basic_block(current_fn, "init_fast");
            let slow_bb = self
                .backend
                .context
                .append_basic_block(current_fn, "init_slow");
            let merge_bb = self
                .backend
                .context
                .append_basic_block(current_fn, "init_merge");
            self.all_llvm_blocks.push(fast_bb);
            self.all_llvm_blocks.push(slow_bb);
            self.all_llvm_blocks.push(merge_bb);
            self.backend
                .builder
                .build_conditional_branch(is_ptr, slow_bb, fast_bb)
                .unwrap();
            // Fast path: immediate value — direct store.
            self.backend.builder.position_at_end(fast_bb);
            let raw_ptr = self
                .backend
                .builder
                .build_int_to_ptr(obj_ptr_bits, ptr_ty, "obj_ptr")
                .unwrap();
            let offset_val = i64_ty.const_int(offset as u64, true);
            let field_ptr = unsafe {
                self.backend
                    .builder
                    .build_in_bounds_gep(i8_ty, raw_ptr, &[offset_val], "field_ptr")
                    .unwrap()
            };
            self.backend
                .builder
                .build_store(field_ptr, val_bits)
                .unwrap();
            self.backend
                .builder
                .build_unconditional_branch(merge_bb)
                .unwrap();
            // Slow path: pointer value — runtime call.
            self.backend.builder.position_at_end(slow_bb);
            let set_fn = self.ensure_runtime_i64_fn("molt_object_field_init_ptr", 3);
            self.backend
                .builder
                .build_call(
                    set_fn,
                    &[
                        obj_ptr_bits.into(),
                        i64_ty.const_int(offset as u64, true).into(),
                        val_bits.into(),
                    ],
                    "field_init_slow",
                )
                .unwrap();
            self.backend
                .builder
                .build_unconditional_branch(merge_bb)
                .unwrap();
            // Merge.
            self.backend.builder.position_at_end(merge_bb);
            if !op.results.is_empty() {
                let none_val: BasicValueEnum<'ctx> = i64_ty
                    .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                    .into();
                self.values.insert(op.results[0], none_val);
                self.value_types.insert(op.results[0], TirType::DynBox);
            }
            return;
        }
        if matches!(original_kind, Some("store")) && op.operands.len() >= 2 {
            let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
            let val_bits = self.materialize_dynbox_operand(op.operands[1]);
            let offset = op
                .attrs
                .get("value")
                .and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                })
                .unwrap_or(0);
            let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
            let set_fn = self.ensure_runtime_i64_fn("molt_object_field_set_ptr", 3);
            let result = self
                .backend
                .builder
                .build_call(
                    set_fn,
                    &[
                        obj_ptr_bits.into(),
                        self.backend
                            .context
                            .i64_type()
                            .const_int(offset as u64, true)
                            .into(),
                        val_bits.into(),
                    ],
                    "field_store",
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            if !op.results.is_empty() {
                self.values.insert(op.results[0], result);
                self.value_types.insert(op.results[0], TirType::DynBox);
            }
            return;
        }
        if matches!(
            original_kind,
            Some("guarded_field_set") | Some("guarded_field_init")
        ) && op.operands.len() >= 4
        {
            let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
            let class_bits = self.materialize_dynbox_operand(op.operands[1]);
            let expected_version = self.materialize_dynbox_operand(op.operands[2]);
            let val_bits = self.materialize_dynbox_operand(op.operands[3]);
            let attr_name = op
                .attrs
                .get("name")
                .and_then(|v| {
                    if let AttrValue::Str(s) = v {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("<unknown>");
            let offset = op
                .attrs
                .get("value")
                .and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                })
                .unwrap_or(0);
            let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
            let (attr_ptr_bits, attr_len_bits) = self.raw_string_const_ptr_len(attr_name);
            let rt_name = if matches!(original_kind, Some("guarded_field_init")) {
                "molt_guarded_field_init_ptr"
            } else {
                "molt_guarded_field_set_ptr"
            };
            let set_fn = self.ensure_runtime_i64_fn(rt_name, 7);
            let result = self
                .backend
                .builder
                .build_call(
                    set_fn,
                    &[
                        obj_ptr_bits.into(),
                        class_bits.into(),
                        expected_version.into(),
                        self.backend
                            .context
                            .i64_type()
                            .const_int(offset as u64, true)
                            .into(),
                        val_bits.into(),
                        attr_ptr_bits.into(),
                        attr_len_bits.into(),
                    ],
                    "guarded_field_set",
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            if !op.results.is_empty() {
                self.values.insert(op.results[0], result);
                self.value_types.insert(op.results[0], TirType::DynBox);
            }
            return;
        }
        let obj = self.resolve(op.operands[0]);
        let attr_name = op
            .attrs
            .get("name")
            .and_then(|v| {
                if let AttrValue::Str(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("<unknown>");
        let name = self.intern_string_const(attr_name);
        let val = self.resolve(op.operands[1]);
        let obj_i64 = self.materialize_dynbox_bits(
            obj,
            &self
                .value_types
                .get(&op.operands[0])
                .cloned()
                .unwrap_or(TirType::DynBox),
        );
        let name_i64 = self.ensure_i64(name);
        let val_i64 = self.materialize_dynbox_bits(
            val,
            &self
                .value_types
                .get(&op.operands[1])
                .cloned()
                .unwrap_or(TirType::DynBox),
        );
        let set_fn = self
            .backend
            .module
            .get_function("molt_set_attr_name")
            .unwrap();
        let result = self
            .backend
            .builder
            .build_call(
                set_fn,
                &[obj_i64.into(), name_i64.into(), val_i64.into()],
                "setattr",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if !op.results.is_empty() {
            self.values.insert(op.results[0], result);
            self.value_types.insert(op.results[0], TirType::DynBox);
        }
    }

    pub(super) fn emit_del_attr(&mut self, op: &TirOp) {
        let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
            AttrValue::Str(s) => Some(s.as_str()),
            _ => None,
        });
        if matches!(original_kind, Some("del_attr_name")) && op.operands.len() >= 2 {
            let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
            let name_bits = self.materialize_dynbox_operand(op.operands[1]);
            let del_fn = self.ensure_runtime_i64_fn("molt_del_attr_name", 2);
            let val = self
                .backend
                .builder
                .build_call(
                    del_fn,
                    &[obj_bits.into(), name_bits.into()],
                    "del_attr_name_dyn",
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            if !op.results.is_empty() {
                self.values.insert(op.results[0], val);
                self.value_types.insert(op.results[0], TirType::DynBox);
            }
            return;
        }
        let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
        let attr_name = op
            .attrs
            .get("name")
            .and_then(|v| {
                if let AttrValue::Str(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("<unknown>");
        let name = self.intern_string_const(attr_name);
        let name_bits = self.ensure_i64(name);
        let del_fn = self.ensure_runtime_i64_fn("molt_del_attr_name", 2);
        let val = self
            .backend
            .builder
            .build_call(
                del_fn,
                &[obj_bits.into(), name_bits.into()],
                "del_attr_name",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if !op.results.is_empty() {
            self.values.insert(op.results[0], val);
            self.value_types.insert(op.results[0], TirType::DynBox);
        }
    }

    pub(super) fn emit_index(&mut self, op: &TirOp) {
        let result_id = op.results[0];
        // BCE: when the bounds-check elimination pass has proven the index
        // is in-range, we call `molt_getitem_unchecked` which skips the
        // runtime bounds check and associated branch entirely.
        let val = if has_attr(op, "bce_safe") {
            self.call_runtime_2_boxed("molt_getitem_unchecked", op.operands[0], op.operands[1])
        } else {
            self.call_runtime_2_boxed("molt_getitem_method", op.operands[0], op.operands[1])
        };
        self.values.insert(result_id, val);
        self.value_types.insert(result_id, TirType::DynBox);
    }

    pub(super) fn emit_store_index(&mut self, op: &TirOp) {
        let obj_i64 = self.materialize_dynbox_operand(op.operands[0]);
        let key_i64 = self.materialize_dynbox_operand(op.operands[1]);
        let val_i64 = self.materialize_dynbox_operand(op.operands[2]);
        let set_fn = self
            .backend
            .module
            .get_function("molt_setitem_method")
            .unwrap();
        let result = self
            .backend
            .builder
            .build_call(
                set_fn,
                &[obj_i64.into(), key_i64.into(), val_i64.into()],
                "setitem",
            )
            .unwrap()
            .try_as_basic_value()
            .unwrap_basic();
        if !op.results.is_empty() {
            self.values.insert(op.results[0], result);
            self.value_types.insert(op.results[0], TirType::DynBox);
        }
    }

    pub(super) fn emit_del_index(&mut self, op: &TirOp) {
        let val = self.call_runtime_2_boxed("molt_delitem_method", op.operands[0], op.operands[1]);
        if !op.results.is_empty() {
            self.values.insert(op.results[0], val);
            self.value_types.insert(op.results[0], TirType::DynBox);
        }
    }
}
