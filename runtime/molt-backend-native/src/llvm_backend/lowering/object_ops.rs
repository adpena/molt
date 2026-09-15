use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    fn field_value_is_pointer(
        &self,
        bits: inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        let i64_ty = self.backend.context.i64_type();
        let tag = self
            .backend
            .builder
            .build_and(
                bits,
                i64_ty.const_int(nanbox::QNAN_TAG_MASK_I64 as u64, false),
                "field_tag",
            )
            .unwrap();
        self.backend
            .builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                tag,
                i64_ty.const_int(nanbox::QNAN_TAG_PTR_I64 as u64, false),
                "field_is_ptr",
            )
            .unwrap()
    }

    /// The caller must have admitted the receiver before this header read.
    /// HAS_PTRS is sticky after instance dictionary materialization, so a clear
    /// bit admits inline backing without duplicating runtime storage metadata.
    fn field_needs_runtime(
        &self,
        obj_ptr_bits: inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::IntValue<'ctx> {
        let builder = &self.backend.builder;
        let i32_ty = self.backend.context.i32_type();
        let i64_ty = self.backend.context.i64_type();
        let ptr_ty = self
            .backend
            .context
            .ptr_type(inkwell::AddressSpace::default());
        let obj_ptr = builder
            .build_int_to_ptr(obj_ptr_bits, ptr_ty, "field_object")
            .unwrap();
        let header_offset =
            i64_ty.const_int(molt_codegen_abi::HEADER_FLAGS_OFFSET as i64 as u64, true);
        let flags_ptr = unsafe {
            builder
                .build_gep(
                    self.backend.context.i8_type(),
                    obj_ptr,
                    &[header_offset],
                    "field_flags_ptr",
                )
                .unwrap()
        };
        let flags = builder
            .build_load(i32_ty, flags_ptr, "field_flags")
            .unwrap()
            .into_int_value();
        let mask = i32_ty.const_int(u64::from(molt_codegen_abi::HEADER_FLAG_HAS_PTRS), false);
        let has_ptrs = builder.build_and(flags, mask, "field_has_ptrs").unwrap();
        builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                has_ptrs,
                i32_ty.const_zero(),
                "field_needs_runtime",
            )
            .unwrap()
    }

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
                    AttrValue::Int(value) => Some(*value),
                    _ => None,
                })
                .unwrap_or(0);
            let obj_ptr_bits = self.unbox_ptr_bits(obj_bits);
            let i64_ty = self.backend.context.i64_type();
            let ptr_ty = self
                .backend
                .context
                .ptr_type(inkwell::AddressSpace::default());
            let receiver_block = self.backend.builder.get_insert_block().unwrap();
            let receiver_bb = self
                .backend
                .context
                .append_basic_block(self.llvm_fn, "field_receiver");
            let load_bb = self
                .backend
                .context
                .append_basic_block(self.llvm_fn, "field_load");
            let runtime_bb = self
                .backend
                .context
                .append_basic_block(self.llvm_fn, "field_runtime");
            let merge_bb = self
                .backend
                .context
                .append_basic_block(self.llvm_fn, "field_load_merge");
            self.all_llvm_blocks
                .extend([receiver_bb, load_bb, runtime_bb, merge_bb]);
            let receiver_is_pointer = self.field_value_is_pointer(obj_bits);
            self.backend
                .builder
                .build_conditional_branch(receiver_is_pointer, receiver_bb, merge_bb)
                .unwrap();

            // Receiver admission dominates both header and payload accesses.
            self.backend.builder.position_at_end(receiver_bb);
            let needs_runtime = self.field_needs_runtime(obj_ptr_bits);
            self.backend
                .builder
                .build_conditional_branch(needs_runtime, runtime_bb, load_bb)
                .unwrap();

            self.backend.builder.position_at_end(load_bb);
            let raw_ptr = self
                .backend
                .builder
                .build_int_to_ptr(obj_ptr_bits, ptr_ty, "obj_ptr")
                .unwrap();
            let offset_val = i64_ty.const_int(offset as u64, true);
            let field_ptr = unsafe {
                self.backend
                    .builder
                    .build_in_bounds_gep(
                        self.backend.context.i8_type(),
                        raw_ptr,
                        &[offset_val],
                        "field_ptr",
                    )
                    .unwrap()
            };
            // A clear HAS_PTRS excludes ordinary heap owners and dictionary
            // backing, but virgin words can contain the immortal missing
            // sentinel. Only an immediate value may return directly.
            let val = self
                .backend
                .builder
                .build_load(i64_ty, field_ptr, "field_val")
                .unwrap();
            let value_is_pointer = self.field_value_is_pointer(val.into_int_value());
            self.backend
                .builder
                .build_conditional_branch(value_is_pointer, runtime_bb, merge_bb)
                .unwrap();

            self.backend.builder.position_at_end(runtime_bb);
            let get_fn = self.ensure_runtime_i64_fn("molt_object_field_get_ptr", 2);
            let runtime_val = self
                .backend
                .builder
                .build_call(
                    get_fn,
                    &[obj_ptr_bits.into(), offset_val.into()],
                    "field_get_runtime",
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic();
            self.backend
                .builder
                .build_unconditional_branch(merge_bb)
                .unwrap();

            self.backend.builder.position_at_end(merge_bb);
            let none = i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false);
            let loaded = self
                .backend
                .builder
                .build_phi(i64_ty, "field_load_value")
                .unwrap();
            loaded.add_incoming(&[
                (&none, receiver_block),
                (&val, load_bb),
                (&runtime_val, runtime_bb),
            ]);
            self.values.insert(result_id, loaded.as_basic_value());
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
            let (attr_ptr_bits, attr_len_bits) = self.raw_string_const_ptr_len(attr_name);
            let get_fn = self.ensure_runtime_i64_fn("molt_guarded_field_get", 6);
            let val = self
                .backend
                .builder
                .build_call(
                    get_fn,
                    &[
                        obj_bits.into(),
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
        let val = if runtime_name == "molt_get_attr_object_ic" {
            let i64_ty = self.backend.context.i64_type();
            let ptr_ty = self
                .backend
                .context
                .ptr_type(inkwell::AddressSpace::default());
            let fn_ty = i64_ty.fn_type(
                &[i64_ty.into(), ptr_ty.into(), i64_ty.into(), i64_ty.into()],
                false,
            );
            let get_fn = declare_fixed_runtime_function(
                self.backend.context,
                &self.backend.module,
                runtime_name,
            )
            .unwrap_or_else(|| panic!("{runtime_name} must be a fixed LLVM runtime import"));
            let get_fn = require_llvm_function_type(runtime_name, get_fn, fn_ty);
            let (attr_ptr, attr_len_bits) = self.raw_string_const_ptr_and_len(attr_name);
            let site_bits = self.source_call_site_bits(op, "get_attr_generic_obj");
            let call_args_generic = [
                obj_bits.into(),
                attr_ptr.into(),
                attr_len_bits.into(),
                site_bits.into(),
            ];
            self.backend
                .builder
                .build_call(get_fn, &call_args_generic, runtime_name)
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
        } else {
            let get_fn = self.ensure_runtime_i64_fn(runtime_name, 2);
            self.with_owned_name(attr_name, |this, name_bits| {
                this.backend
                    .builder
                    .build_call(get_fn, &[obj_bits.into(), name_bits.into()], runtime_name)
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
            })
        };
        // Runtime getattr entry points return one owned result on every
        // successful path, including IC hits. Preserve that single authority;
        // a backend-side retain would leak bound-method receivers.
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
            // A scalar write can be inline only while physical backing has
            // no pointer owners or dictionary and the incoming value is immediate.
            // Every other write uses the ordinary retain/publish/release contract.
            let i64_ty = self.backend.context.i64_type();
            let i8_ty = self.backend.context.i8_type();
            let ptr_ty = self
                .backend
                .context
                .ptr_type(inkwell::AddressSpace::default());
            let is_ptr = self.field_value_is_pointer(val_bits);
            let receiver_is_ptr = self.field_value_is_pointer(obj_bits);
            let current_fn = self.llvm_fn;
            let receiver_bb = self
                .backend
                .context
                .append_basic_block(current_fn, "field_store_receiver");
            let fast_bb = self
                .backend
                .context
                .append_basic_block(current_fn, "field_store_fast");
            let slow_bb = self
                .backend
                .context
                .append_basic_block(current_fn, "field_store_slow");
            let merge_bb = self
                .backend
                .context
                .append_basic_block(current_fn, "field_store_merge");
            self.all_llvm_blocks.push(fast_bb);
            self.all_llvm_blocks.push(slow_bb);
            self.all_llvm_blocks.push(merge_bb);
            self.all_llvm_blocks.push(receiver_bb);
            self.backend
                .builder
                .build_conditional_branch(receiver_is_ptr, receiver_bb, slow_bb)
                .unwrap();
            self.backend.builder.position_at_end(receiver_bb);
            let backing_needs_runtime = self.field_needs_runtime(obj_ptr_bits);
            let needs_runtime = self
                .backend
                .builder
                .build_or(is_ptr, backing_needs_runtime, "field_store_needs_runtime")
                .unwrap();
            self.backend
                .builder
                .build_conditional_branch(needs_runtime, slow_bb, fast_bb)
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
            // Slow path: owning value or dictionary backing — runtime call.
            self.backend.builder.position_at_end(slow_bb);
            let set_fn = self.ensure_runtime_i64_fn("molt_object_field_set", 3);
            self.backend
                .builder
                .build_call(
                    set_fn,
                    &[
                        obj_bits.into(),
                        i64_ty.const_int(offset as u64, true).into(),
                        val_bits.into(),
                    ],
                    "field_store_slow",
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
        if matches!(original_kind, Some("guarded_field_set")) && op.operands.len() >= 4 {
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
            let (attr_ptr_bits, attr_len_bits) = self.raw_string_const_ptr_len(attr_name);
            let set_fn = self.ensure_runtime_i64_fn("molt_guarded_field_set", 7);
            let result = self
                .backend
                .builder
                .build_call(
                    set_fn,
                    &[
                        obj_bits.into(),
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
        let val = self.resolve(op.operands[1]);
        let obj_i64 = self.materialize_dynbox_bits(
            obj,
            &self
                .value_types
                .get(&op.operands[0])
                .cloned()
                .unwrap_or(TirType::DynBox),
        );
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
        let result = self.with_owned_name(attr_name, |this, name_i64| {
            this.backend
                .builder
                .build_call(
                    set_fn,
                    &[obj_i64.into(), name_i64.into(), val_i64.into()],
                    "setattr",
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
        });
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
        let del_fn = self.ensure_runtime_i64_fn("molt_del_attr_name", 2);
        let val = self.with_owned_name(attr_name, |this, name_bits| {
            this.backend
                .builder
                .build_call(
                    del_fn,
                    &[obj_bits.into(), name_bits.into()],
                    "del_attr_name",
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
        });
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
