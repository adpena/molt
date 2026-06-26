use super::*;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn lower_op(&mut self, source_block: BlockId, op: &crate::tir::ops::TirOp) {
        match op.opcode {
            // ── Constants ──
            OpCode::ConstInt => self.emit_const_int(op),
            OpCode::ConstFloat => self.emit_const_float(op),
            OpCode::ConstBool => self.emit_const_bool(op),
            OpCode::ConstNone => self.emit_const_none(op),
            OpCode::ConstStr => self.emit_const_str(op),
            OpCode::ConstBigInt => self.emit_const_bigint(op),
            OpCode::ConstBytes => self.emit_const_bytes(op),

            // ── Arithmetic (type-specialized) ──
            OpCode::Add | OpCode::InplaceAdd => self.emit_binary_arith(op, "add"),
            OpCode::CheckedAdd => self.emit_checked_add(op),
            OpCode::CheckedMul => self.emit_checked_mul(op),
            OpCode::Sub | OpCode::InplaceSub => self.emit_binary_arith(op, "sub"),
            OpCode::Mul | OpCode::InplaceMul => self.emit_binary_arith(op, "mul"),
            OpCode::Div => self.emit_binary_arith(op, "div"),
            OpCode::FloorDiv => self.emit_binary_arith(op, "floordiv"),
            OpCode::Mod => self.emit_binary_arith(op, "mod"),
            OpCode::Pow => self.emit_binary_arith(op, "pow"),

            // ── Unary ──
            OpCode::Neg => self.emit_unary(op, "neg"),
            OpCode::Pos => {
                // Pos is identity for numeric types.
                let result_id = op.results[0];
                let operand = op.operands[0];
                let val = self.values[&operand];
                let ty = self.value_types[&operand].clone();
                self.values.insert(result_id, val);
                self.value_types.insert(result_id, ty);
            }
            OpCode::Not => self.emit_unary(op, "not"),

            // ── Comparison (type-specialized) ──
            OpCode::Eq => self.emit_comparison(op, "eq"),
            OpCode::Ne => self.emit_comparison(op, "ne"),
            OpCode::Lt => self.emit_comparison(op, "lt"),
            OpCode::Le => self.emit_comparison(op, "le"),
            OpCode::Gt => self.emit_comparison(op, "gt"),
            OpCode::Ge => self.emit_comparison(op, "ge"),
            OpCode::Is | OpCode::IsNot => self.emit_identity(op),
            OpCode::In | OpCode::NotIn => self.emit_containment(op),

            // ── Bitwise ──
            OpCode::BitAnd => self.emit_bitwise(op, "bit_and"),
            OpCode::BitOr => self.emit_bitwise(op, "bit_or"),
            OpCode::BitXor => self.emit_bitwise(op, "bit_xor"),
            OpCode::BitNot => self.emit_unary(op, "invert"),
            OpCode::Shl => self.emit_bitwise(op, "lshift"),
            OpCode::Shr => self.emit_bitwise(op, "rshift"),

            // ── Boolean ──
            OpCode::And | OpCode::Or => {
                // Frontend BoolOp lowering uses And/Or ops to produce the
                // selected operand value inside already-structured control flow.
                // At this stage we must preserve Python operand-selection
                // semantics, not bitwise semantics.
                let result_id = op.results[0];
                let lhs = self.resolve(op.operands[0]);
                let rhs = self.resolve(op.operands[1]);
                let lhs_ty = self
                    .value_types
                    .get(&op.operands[0])
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                let rhs_ty = self
                    .value_types
                    .get(&op.operands[1])
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                let lhs_i64 = self.ensure_i64(lhs);
                let truthy_fn = self.backend.module.get_function("molt_is_truthy").unwrap();
                let truthy = self
                    .backend
                    .builder
                    .build_call(truthy_fn, &[lhs_i64.into()], "truthy")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();
                let cond_i1 = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        truthy,
                        self.backend.context.i64_type().const_zero(),
                        "boolop_cond",
                    )
                    .unwrap();
                let lhs_bits = self.materialize_dynbox_bits(lhs, &lhs_ty);
                let rhs_bits = self.materialize_dynbox_bits(rhs, &rhs_ty);
                let selected = if op.opcode == OpCode::And {
                    self.backend
                        .builder
                        .build_select(cond_i1, rhs_bits, lhs_bits, "bool_and")
                        .unwrap()
                } else {
                    self.backend
                        .builder
                        .build_select(cond_i1, lhs_bits, rhs_bits, "bool_or")
                        .unwrap()
                };
                if crate::tir::op_kinds_generated::opcode_result_mints_owned_selected_operand_table(
                    op.opcode,
                ) {
                    let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
                    self.backend
                        .builder
                        .build_call(
                            inc_fn,
                            &[selected.into_int_value().into()],
                            "boolop_selected_inc_ref",
                        )
                        .unwrap();
                }
                self.values.insert(result_id, selected);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::Bool => {
                let result_id = op.results[0];
                let operand_id = op.operands[0];
                let operand = self.resolve(operand_id);
                let operand_ty = self
                    .value_types
                    .get(&operand_id)
                    .cloned()
                    .unwrap_or(TirType::DynBox);

                let bool_val = match operand_ty {
                    TirType::Bool => operand.into_int_value(),
                    TirType::I64 => self
                        .backend
                        .builder
                        .build_int_compare(
                            inkwell::IntPredicate::NE,
                            operand.into_int_value(),
                            self.backend.context.i64_type().const_zero(),
                            "bool_i64",
                        )
                        .unwrap(),
                    TirType::F64 => self
                        .backend
                        .builder
                        .build_float_compare(
                            inkwell::FloatPredicate::ONE,
                            operand.into_float_value(),
                            self.backend.context.f64_type().const_float(0.0),
                            "bool_f64",
                        )
                        .unwrap(),
                    _ => {
                        let truthy_fn = self.backend.module.get_function("molt_is_truthy").unwrap();
                        let truthy = self
                            .backend
                            .builder
                            .build_call(
                                truthy_fn,
                                &[self.ensure_i64(operand).into()],
                                "truthy_bool",
                            )
                            .unwrap()
                            .try_as_basic_value()
                            .unwrap_basic()
                            .into_int_value();
                        self.backend
                            .builder
                            .build_int_compare(
                                inkwell::IntPredicate::NE,
                                truthy,
                                self.backend.context.i64_type().const_zero(),
                                "bool_dynbox",
                            )
                            .unwrap()
                    }
                };
                self.values.insert(result_id, bool_val.into());
                self.value_types.insert(result_id, TirType::Bool);
            }

            // ── Box/Unbox ──
            OpCode::BoxVal => self.emit_box(op),
            OpCode::UnboxVal => self.emit_unbox(op),
            OpCode::TypeGuard => {
                // Type guard: in lowered code, this is a no-op assertion.
                // The value passes through; if the guard fails at runtime,
                // deopt kicks in (handled elsewhere).
                let result_id = op.results[0];
                let val = self.resolve(op.operands[0]);
                self.values.insert(result_id, val);
                let ty = self
                    .value_types
                    .get(&op.operands[0])
                    .cloned()
                    .unwrap_or(TirType::DynBox);
                self.value_types.insert(result_id, ty);
            }

            // ── Refcount ──
            OpCode::IncRef => {
                let val = self.resolve(op.operands[0]);
                let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
                let bits = self.ensure_i64(val);
                self.backend
                    .builder
                    .build_call(inc_fn, &[bits.into()], "")
                    .unwrap();
                // IncRef has no result, but if it does, pass through.
                if !op.results.is_empty() {
                    self.values.insert(op.results[0], val);
                    let ty = self
                        .value_types
                        .get(&op.operands[0])
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    self.value_types.insert(op.results[0], ty);
                }
            }
            // Python lifetime boundary (#58): when a `del`/rebind/scope-exit
            // marker survives to LLVM, it is the release authority for that
            // named-local owner. The drop phase may rewrite some markers to
            // `DecRef`; any marker left here must still lower to the same
            // runtime release, matching the native backend's direct
            // `del_boundary` arm.
            OpCode::DelBoundary => {
                let val = self.resolve(op.operands[0]);
                let bits = self.ensure_i64(val);
                let dec_fn = self.ensure_runtime_void_fn("molt_dec_ref_obj", 1);
                self.backend
                    .builder
                    .build_call(dec_fn, &[bits.into()], "")
                    .unwrap();
            }
            OpCode::DecRef => {
                let val = self.resolve(op.operands[0]);
                let dec_fn = self.ensure_runtime_void_fn("molt_dec_ref_obj", 1);
                let bits = self.ensure_i64(val);
                self.backend
                    .builder
                    .build_call(dec_fn, &[bits.into()], "")
                    .unwrap();
            }

            // ── Memory / Attribute / Index ──
            OpCode::LoadAttr => {
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
            OpCode::StoreAttr => {
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
            OpCode::DelAttr => {
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
            OpCode::Index => {
                let result_id = op.results[0];
                // BCE: when the bounds-check elimination pass has proven the index
                // is in-range, we call `molt_getitem_unchecked` which skips the
                // runtime bounds check and associated branch entirely.
                let val = if has_attr(op, "bce_safe") {
                    self.call_runtime_2_boxed(
                        "molt_getitem_unchecked",
                        op.operands[0],
                        op.operands[1],
                    )
                } else {
                    self.call_runtime_2_boxed("molt_getitem_method", op.operands[0], op.operands[1])
                };
                self.values.insert(result_id, val);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::StoreIndex => {
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
            OpCode::DelIndex => {
                let val = self.call_runtime_2_boxed(
                    "molt_delitem_method",
                    op.operands[0],
                    op.operands[1],
                );
                if !op.results.is_empty() {
                    self.values.insert(op.results[0], val);
                    self.value_types.insert(op.results[0], TirType::DynBox);
                }
            }

            // ── Call ──
            OpCode::Call => {
                let i64_ty = self.backend.context.i64_type();
                let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                });

                if matches!(original_kind, Some("call_func") | Some("call_function"))
                    && !op.operands.is_empty()
                {
                    let callable = self.resolve(op.operands[0]);
                    let result = self.emit_call_func_or_bind_runtime(callable, &op.operands[1..]);
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                    return;
                }

                // Direct call by name: call_guarded stores the target function
                // name in s_value / _var, with all operands being arguments
                // (not a callable reference).  If the target already exists in
                // the LLVM module (same compilation unit), call it directly.
                let direct_target: Option<String> = op
                    .attrs
                    .get("s_value")
                    .or_else(|| op.attrs.get("_var"))
                    .and_then(|v| match v {
                        AttrValue::Str(s) if !s.is_empty() => Some(s.clone()),
                        _ => None,
                    });
                let direct_operands: &[ValueId] = if matches!(original_kind, Some("call_guarded")) {
                    op.operands.get(1..).unwrap_or(&[])
                } else {
                    &op.operands
                };
                let guarded_callable = if matches!(original_kind, Some("call_guarded")) {
                    op.operands.first().copied()
                } else {
                    None
                };

                if matches!(original_kind, Some("call_bind") | Some("call_indirect"))
                    && op.operands.len() >= 2
                {
                    let callable_i64 = self.ensure_i64(self.resolve(op.operands[0]));
                    let builder_bits = self.ensure_i64(self.resolve(op.operands[1]));
                    let site_bits = self.next_call_site_bits(original_kind.unwrap_or("call_bind"));
                    let runtime_name = if matches!(original_kind, Some("call_indirect")) {
                        "molt_call_indirect_ic"
                    } else {
                        "molt_call_bind_ic"
                    };
                    let runtime_fn = self.ensure_runtime_i64_fn(runtime_name, 3);
                    let result = self
                        .backend
                        .builder
                        .build_call(
                            runtime_fn,
                            &[site_bits.into(), callable_i64.into(), builder_bits.into()],
                            runtime_name,
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                    return;
                }

                if matches!(original_kind, Some("call_guarded"))
                    && let Some(callable_id) = guarded_callable
                {
                    let callable = self.resolve(callable_id);
                    let result = self.emit_call_func_runtime(callable, direct_operands);
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                    return;
                }

                if let Some(ref target_name) = direct_target {
                    if let Some(target_fn) = self.backend.module.get_function(target_name) {
                        let target_return_tir_ty = self
                            .backend
                            .function_return_types
                            .get(target_name.as_str())
                            .cloned()
                            .unwrap_or(TirType::DynBox);
                        let expected_params = target_fn.count_params() as usize;
                        if expected_params != direct_operands.len()
                            && let Some(callable_id) = guarded_callable
                        {
                            let callable = self.resolve(callable_id);
                            let result = self.emit_call_bind_runtime(callable, direct_operands);
                            if let Some(&result_id) = op.results.first() {
                                self.values.insert(result_id, result);
                                self.value_types.insert(result_id, TirType::DynBox);
                            }
                            return;
                        }
                        let current_bb = self
                            .backend
                            .builder
                            .get_insert_block()
                            .expect("direct call must be emitted inside a basic block");
                        // Direct call — all operands are positional args.
                        // Every direct-call argument must be coerced from its
                        // SOURCE TirType to the CALLEE's declared param TirType
                        // (DynBox = the boxed molt ABI default). This was
                        // previously gated on `call_guarded` only — a plain
                        // `call`/`call_internal` passed an I64-typed value (or
                        // constant) RAW into a NaN-boxed parameter, where the
                        // raw bits decode as a garbage float (e.g.
                        // `compute(1000000)` received ~4.9e-318 and the loop
                        // exited after one iteration). The LLVM-type coercion
                        // below is a bitcast-level cast and cannot substitute
                        // for representation boxing.
                        let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
                            direct_operands
                                .iter()
                                .enumerate()
                                .map(|(idx, &id)| {
                                    let v = self.resolve(id);
                                    let source_tir_ty = self
                                        .value_types
                                        .get(&id)
                                        .cloned()
                                        .unwrap_or(TirType::DynBox);
                                    let target_tir_ty = self
                                        .backend
                                        .function_param_types
                                        .get(target_name.as_str())
                                        .and_then(|tys| tys.get(idx))
                                        .cloned()
                                        .unwrap_or(TirType::DynBox);
                                    let v = self.coerce_to_tir_type(
                                        v,
                                        &source_tir_ty,
                                        &target_tir_ty,
                                        current_bb,
                                    );
                                    let target_ty = target_fn
                                        .get_nth_param(idx as u32)
                                        .map(|param| param.get_type())
                                        .unwrap_or_else(|| self.backend.context.i64_type().into());
                                    self.coerce_to_type(v, target_ty, current_bb).into()
                                })
                                .collect();
                        let call_result = self
                            .backend
                            .builder
                            .build_call(target_fn, &args, "direct_call")
                            .unwrap();
                        if let Some(&result_id) = op.results.first() {
                            let raw_result =
                                call_result.try_as_basic_value().basic().unwrap_or_else(|| {
                                    i64_ty
                                        .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                                        .into()
                                });
                            let result = if target_return_tir_ty == TirType::DynBox {
                                raw_result
                            } else {
                                materialize_dynbox_bits_with_builder(
                                    &self.backend.builder,
                                    self.backend.context,
                                    &self.backend.module,
                                    self.llvm_fn,
                                    raw_result,
                                    &target_return_tir_ty,
                                )
                                .into()
                            };
                            self.values.insert(result_id, result);
                            self.value_types.insert(result_id, TirType::DynBox);
                        }
                    } else {
                        if let Some(callable_id) = guarded_callable {
                            let callable = self.resolve(callable_id);
                            let result = self.emit_call_bind_runtime(callable, direct_operands);
                            if let Some(&result_id) = op.results.first() {
                                self.values.insert(result_id, result);
                                self.value_types.insert(result_id, TirType::DynBox);
                            }
                            return;
                        }
                        // Target not yet in module — forward-declare it and call.
                        let param_types: Vec<inkwell::types::BasicMetadataTypeEnum<'ctx>> =
                            direct_operands.iter().map(|_| i64_ty.into()).collect();
                        let fn_ty = i64_ty.fn_type(&param_types, false);
                        let target_fn = self.backend.module.add_function(
                            target_name,
                            fn_ty,
                            Some(inkwell::module::Linkage::External),
                        );
                        let current_bb = self
                            .backend
                            .builder
                            .get_insert_block()
                            .expect("direct call must be emitted inside a basic block");
                        // Forward-declared target: the callee's TIR param
                        // types are unknown, so the boxed molt ABI (DynBox) is
                        // the contract — box every non-DynBox source (see the
                        // resolved-target path above for the raw-bits-as-float
                        // miscompile this prevents).
                        let args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
                            direct_operands
                                .iter()
                                .enumerate()
                                .map(|(idx, &id)| {
                                    let v = self.resolve(id);
                                    let source_tir_ty = self
                                        .value_types
                                        .get(&id)
                                        .cloned()
                                        .unwrap_or(TirType::DynBox);
                                    let v = self.coerce_to_tir_type(
                                        v,
                                        &source_tir_ty,
                                        &TirType::DynBox,
                                        current_bb,
                                    );
                                    let target_ty = target_fn
                                        .get_nth_param(idx as u32)
                                        .map(|param| param.get_type())
                                        .unwrap_or_else(|| self.backend.context.i64_type().into());
                                    self.coerce_to_type(v, target_ty, current_bb).into()
                                })
                                .collect();
                        let result = self
                            .backend
                            .builder
                            .build_call(target_fn, &args, "direct_call")
                            .unwrap()
                            .try_as_basic_value()
                            .basic()
                            .unwrap_or_else(|| {
                                i64_ty
                                    .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                                    .into()
                            });
                        if let Some(&result_id) = op.results.first() {
                            self.values.insert(result_id, result);
                            self.value_types.insert(result_id, TirType::DynBox);
                        }
                    }
                } else if !op.operands.is_empty() {
                    // Indirect call: operands[0] = callable, rest = positional args.
                    let callable = self.resolve(op.operands[0]);
                    let result = self.emit_call_bind_runtime(callable, &op.operands[1..]);

                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                } else {
                    // No operands, no direct target — emit None.
                    if let Some(&result_id) = op.results.first() {
                        let none_val: BasicValueEnum<'ctx> = i64_ty
                            .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                            .into();
                        self.values.insert(result_id, none_val);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                }
            }

            // ── DeleteVar local-slot transition ──
            // DeleteVar defines the local's new SSA value as the missing sentinel
            // operand. Ownership of the previous slot occupant is modeled by the
            // drop fact plane (DecRef / explicit consumed operands), not by LLVM
            // lowering.
            OpCode::DeleteVar => {
                if op.results.is_empty() {
                    // Side-effect-only legacy shapes have no SSA value to bind.
                } else if let Some(&missing) = op.operands.first() {
                    let val = self.resolve(missing);
                    let ty = self
                        .value_types
                        .get(&missing)
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    for &result_id in &op.results {
                        self.values.insert(result_id, val);
                        self.value_types.insert(result_id, ty.clone());
                    }
                } else {
                    let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                    let none_val: BasicValueEnum<'ctx> = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(none_bits, false)
                        .into();
                    for &result_id in &op.results {
                        self.values.insert(result_id, none_val);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                }
            }

            // ── SSA Copy ──
            // Also serves as the fallback for unknown frontend ops that were
            // mapped to Copy by the SSA converter.  Handle all combinations of
            // operand/result counts gracefully:
            //   - 0 operands, 0 results: no-op (side-effect only)
            //   - 0 operands, 1+ results: produce NaN-boxed None per result
            //   - 1+ operands, 0 results: no-op (side-effect only)
            //   - 1+ operands, 1+ results: pass-through first operand
            OpCode::Copy => {
                let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                });
                if let Some(kind) = original_kind
                    && self.lower_preserved_simpleir_op(op, kind)
                {
                    return;
                }
                // TERMINAL FAIL-LOUD STATE (preserved-op passthrough class),
                // tied to the `CopyLowering` single source of truth.
                //
                // Reaching here means this `OpCode::Copy` carries an
                // `_original_kind` that `lower_preserved_simpleir_op` did NOT
                // handle — neither a dedicated arm nor the generic
                // `try_lower_preserved_runtime_call` (`molt_<kind>`) fallback
                // claimed it. EVERY such op is a SimpleIR op the native/WASM/Luau
                // lanes lower with dedicated semantics (a value-producing runtime
                // call, an RC adjustment, a type guard, a generator throw, or a
                // registry-owned identity fact). Passing operand 0 through by
                // default -- the historical behavior -- silently miscompiled
                // non-identity semantics: `abs(x)` returned `x`,
                // `...`/`NotImplemented` became `None`, `raise ... from ...`'s
                // `__cause__` link, `gen.throw`, special-attr loads, the
                // `borrow`/`release` refcount ops, the `guard_tag` type check, and
                // every fresh-value conversion (`int(x)`, `s[-5:]`, `dict.keys()`,
                // ...) were all DROPPED or returned operand 0. The few preserved
                // repr-identity ops whose operand-0 passthrough is correct are
                // claimed explicitly in `lower_preserved_simpleir_op`; any
                // preserved op that reaches this terminal state is therefore not a
                // sound default passthrough. Fail the build loudly here.
                //
                // SINGLE SOURCE OF TRUTH (the drift the `CopyLowering` classifier
                // forbids). The drop-insertion pass releases exactly the `Copy`s
                // whose `_original_kind` is a `CopyLowering::FreshValue`
                // (`alias_analysis::copy_kind_mints_fresh_owned_ref`). If such a
                // fresh-owned producer reached codegen as a silent operand-0
                // passthrough, the result would (a) be the wrong value AND (b)
                // alias operand 0 — which the drop pass then DOUBLE-FREES. The gate
                // therefore consults that classifier on every fatal so the table
                // and the backend cannot drift: a `FreshValue` reaching here is the
                // forbidden drift (a fresh-value op missing its explicit LLVM arm),
                // and the diagnostic names it as such; any other `_original_kind`
                // gets the general terminal message with operand/result counts.
                // Closing a kind = add an arm to `lower_preserved_simpleir_op` (or,
                // if `molt_<kind>` is a real boxed runtime intrinsic, the generic
                // fallback already covers it). See the `CopyLowering` docs and
                // `tests::copy_lowering_classes_are_total_and_disjoint`.
                if let Some(kind) = original_kind {
                    if crate::tir::passes::alias_analysis::copy_kind_reaches_no_incref_passthrough(
                        Some(kind),
                    ) {
                        // Not a `FreshValue` (a transparent-alias / inert-marker
                        // kind whose `molt_<kind>` intrinsic is also absent): the
                        // partner's general terminal state. Still fail loud — an
                        // unhandled `_original_kind` is never a sound passthrough.
                        self.record_fatal(format!(
                            "unhandled preserved SimpleIR op `{kind}` (operands={}, \
                             results={}) reached the LLVM `Copy` passthrough — \
                             lowering it as a copy of operand 0 would silently \
                             miscompile or drop its side effect; add a \
                             `lower_preserved_simpleir_op` arm for it (or confirm \
                             `molt_{kind}` is a boxed runtime intrinsic so the \
                             generic fallback claims it)",
                            op.operands.len(),
                            op.results.len(),
                        ));
                    } else {
                        // A `CopyLowering::FreshValue` reached the passthrough: the
                        // exact classifier↔backend drift this gate exists to catch.
                        self.record_fatal(format!(
                            "fresh-value SimpleIR op `{kind}` (operands={}, \
                             results={}) reached the LLVM `Copy` passthrough — \
                             lowering it as a copy of operand 0 would silently \
                             miscompile AND make the result alias operand 0 (a \
                             drop-insertion double-free); it is in \
                             `alias_analysis::copy_kind_mints_fresh_owned_ref` so it \
                             MUST have a `lower_preserved_simpleir_op` arm (the \
                             classifier and the LLVM lowering have drifted)",
                            op.operands.len(),
                            op.results.len(),
                        ));
                    }
                    return;
                }

                // `_original_kind == None`: a genuine SSA value copy
                // (`copy`/`copy_var`/`load_var`/`store_var`). Operand-0
                // passthrough is the correct lowering.
                if op.results.is_empty() {
                    // No results — nothing to bind; skip.
                } else if op.operands.is_empty() {
                    // Unknown op with no operands — produce None for each result.
                    let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                    let none_val: BasicValueEnum<'ctx> = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(none_bits, false)
                        .into();
                    for &result_id in &op.results {
                        self.values.insert(result_id, none_val);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                } else {
                    // Standard copy: pass through first operand.
                    let val = self.resolve(op.operands[0]);
                    let ty = self
                        .value_types
                        .get(&op.operands[0])
                        .cloned()
                        .unwrap_or(TirType::DynBox);
                    for &result_id in &op.results {
                        self.values.insert(result_id, val);
                        self.value_types.insert(result_id, ty.clone());
                    }
                }
            }

            // ── Allocation ──
            OpCode::Alloc => {
                let result_id = op.results[0];
                let size = self.resolve(op.operands[0]);
                let size_i64 = self.ensure_i64(size);
                let alloc_fn = self.backend.module.get_function("molt_alloc").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(alloc_fn, &[size_i64.into()], "alloc")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }

            // ── CallMethod: receiver.method(args...) ──
            // Protocol: molt_call_method(receiver, method_name_bits, args_builder) -> u64
            // operands: [receiver, method_name, arg0, arg1, ...]
            OpCode::CallMethod => {
                let i64_ty = self.backend.context.i64_type();
                if op.operands.is_empty() {
                    return;
                }
                let method_bits = self.ensure_i64(self.resolve(op.operands[0]));

                // Build positional args (operands[1..]) for the bound method object.
                let n_args = op.operands.len().saturating_sub(1) as u64;
                let new_fn = self
                    .backend
                    .module
                    .get_function("molt_callargs_new")
                    .unwrap();
                let args_builder = self
                    .backend
                    .builder
                    .build_call(
                        new_fn,
                        &[
                            i64_ty.const_int(n_args, false).into(),
                            i64_ty.const_int(0, false).into(),
                        ],
                        "cm_args",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self
                    .backend
                    .module
                    .get_function("molt_callargs_push_pos")
                    .unwrap();
                for &arg_id in op.operands.get(1..).unwrap_or(&[]) {
                    // Method-call args flow through `molt_call_bind_ic` into the
                    // bound method's trampoline, which decodes each NaN-boxed
                    // `DynBox` into its parameter's raw representation. Box per the
                    // value's representation plan rather than passing raw bits (a
                    // raw `I64`/`F64` arg would be decoded as a boxed payload —
                    // the same carrier miscompile as the plain-call paths).
                    let arg_i64 = self.materialize_dynbox_operand(arg_id);
                    self.backend
                        .builder
                        .build_call(push_fn, &[args_builder.into(), arg_i64.into()], "cm_push")
                        .unwrap();
                }
                let site_bits = self.next_call_site_bits("call_method");
                let call_bind_fn = self.ensure_runtime_i64_fn("molt_call_bind_ic", 3);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        call_bind_fn,
                        &[site_bits.into(), method_bits.into(), args_builder.into()],
                        "call_method_bind",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            OpCode::CallMethodIc => {
                if !self.lower_call_method_ic_op(op) {
                    self.record_fatal(
                        "malformed CallMethodIc op: expected receiver operand and method attr",
                    );
                }
            }

            OpCode::CallSuperMethodIc => {
                if !self.lower_call_super_method_ic_op(op) {
                    self.record_fatal(
                        "malformed CallSuperMethodIc op: expected class/self operands and method attr",
                    );
                }
            }

            // CallBuiltin: builtin_name(args...)
            //
            // Two patterns reach here:
            //   A) `call_builtin` from the frontend: s_value / name attr holds the
            //      builtin name, operands[0] is a ConstStr with the name bits,
            //      rest are positional args.
            //   B) `print` / `builtin_print`: the op kind IS the builtin name,
            //      stored in `_original_kind`. ALL operands are arguments; the
            //      first is NOT a name.
            //
            // We detect (B) by checking for `_original_kind` (only set when the
            // SSA converter wraps a non-canonical kind). For (A), the `name`
            // attr holds the builtin name string.
            OpCode::CallBuiltin => {
                let i64_ty = self.backend.context.i64_type();

                // Determine the builtin name and where positional args start.
                let (builtin_name_str, args_start): (Option<String>, usize) = {
                    let original_kind = op.attrs.get("_original_kind").and_then(|v| match v {
                        AttrValue::Str(s) => Some(s.as_str()),
                        _ => None,
                    });
                    let name_attr = op.attrs.get("name").and_then(|v| match v {
                        AttrValue::Str(s) => Some(s.as_str()),
                        _ => None,
                    });
                    if let Some(kind) = original_kind {
                        // Pattern B: print, builtin_print, etc.
                        // All operands are args.
                        (Some(kind.to_string()), 0)
                    } else if let Some(name) = name_attr {
                        // Pattern A: call_builtin with explicit name.
                        // operands[0] is the name ConstStr, rest are args.
                        (Some(name.to_string()), 1)
                    } else {
                        // Fallback: operands[0] is the name bits.
                        (None, 1)
                    }
                };

                if builtin_name_str.as_deref() == Some("print")
                    || builtin_name_str.as_deref() == Some("builtin_print")
                {
                    // PRINT is a dedicated frontend op. By the time it reaches
                    // backend IR, multi-argument CPython semantics have already
                    // been normalized into a single joined display string plus
                    // explicit newline behavior. Lower it directly to the
                    // runtime print surface just like the native backend.
                    let print_fn = self.ensure_runtime_void_fn("molt_print_obj", 1);
                    for &arg_id in op.operands.get(args_start..).unwrap_or(&[]) {
                        let arg_i64 = self.materialize_dynbox_operand(arg_id);
                        self.backend
                            .builder
                            .build_call(print_fn, &[arg_i64.into()], "print")
                            .unwrap();
                    }
                    if let Some(&result_id) = op.results.first() {
                        let none_val: BasicValueEnum<'ctx> = i64_ty
                            .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                            .into();
                        self.values.insert(result_id, none_val);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                } else if builtin_name_str.as_deref() == Some("range_new") {
                    // `range(...)` is a dedicated frontend op (`RANGE_NEW`), not a
                    // generic builtin lookup. The SSA lifter folds it into
                    // `OpCode::CallBuiltin` with `_original_kind = "range_new"`
                    // (ssa.rs), but `range` is NOT registered as a runtime
                    // intrinsic and `molt_call_builtin` would fall through to the
                    // builtins module-cache path — failing at any call site reached
                    // before that cache is populated. Lower directly to the
                    // dedicated runtime constructor `molt_range_new(start, stop,
                    // step)`, exactly as the native and WASM backends do. The
                    // frontend (`_parse_range_call`) always materializes all three
                    // boxed bounds (start defaults to 0, step to 1), so operands is
                    // exactly [start, stop, step] (args_start == 0 because Pattern B
                    // was detected via `_original_kind`).
                    debug_assert_eq!(
                        op.operands.len(),
                        3,
                        "range_new must carry exactly [start, stop, step]"
                    );
                    if op.operands.len() != 3 {
                        return;
                    }
                    let range_new_fn = self.ensure_runtime_i64_fn("molt_range_new", 3);
                    let start = self.materialize_dynbox_operand(op.operands[0]).into();
                    let stop = self.materialize_dynbox_operand(op.operands[1]).into();
                    let step = self.materialize_dynbox_operand(op.operands[2]).into();
                    let result = self
                        .backend
                        .builder
                        .build_call(range_new_fn, &[start, stop, step], "range_new")
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                } else {
                    // Generic builtin call via molt_call_builtin.
                    let builtin_name_bits = if let Some(ref name) = builtin_name_str {
                        // Create a runtime string for the builtin name via
                        // molt_string_from_bytes.
                        let name_val = self.intern_string_const(name);
                        self.ensure_i64(name_val)
                    } else if args_start <= op.operands.len() && !op.operands.is_empty() {
                        let bv = self.resolve(op.operands[0]);
                        self.ensure_i64(bv)
                    } else if let Some(s_val) = op.attrs.get("s_value").and_then(|v| {
                        if let AttrValue::Str(s) = v {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    }) {
                        let name_val = self.intern_string_const(s_val);
                        self.ensure_i64(name_val)
                    } else {
                        i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                    };

                    let n_args = op.operands.len().saturating_sub(args_start) as u64;
                    let new_fn = self
                        .backend
                        .module
                        .get_function("molt_callargs_new")
                        .unwrap();
                    let args_builder = self
                        .backend
                        .builder
                        .build_call(
                            new_fn,
                            &[
                                i64_ty.const_int(n_args, false).into(),
                                i64_ty.const_int(0, false).into(),
                            ],
                            "cb_args",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    let push_fn = self
                        .backend
                        .module
                        .get_function("molt_callargs_push_pos")
                        .unwrap();
                    for &arg_id in op.operands.get(args_start..).unwrap_or(&[]) {
                        let arg_i64 = self.materialize_dynbox_operand(arg_id);
                        self.backend
                            .builder
                            .build_call(push_fn, &[args_builder.into(), arg_i64.into()], "cb_push")
                            .unwrap();
                    }

                    let call_builtin_fn = self
                        .backend
                        .module
                        .get_function("molt_call_builtin")
                        .unwrap();
                    let result = self
                        .backend
                        .builder
                        .build_call(
                            call_builtin_fn,
                            &[builtin_name_bits.into(), args_builder.into()],
                            "call_builtin",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                }
            }

            // ── OrdAt: fused ord(container[index]) ──
            OpCode::OrdAt => {
                if op.operands.len() < 2 {
                    return;
                }
                let obj_bits = self.materialize_dynbox_operand(op.operands[0]);
                let index_bits = self.materialize_dynbox_operand(op.operands[1]);
                let ord_at_fn = self.ensure_runtime_i64_fn("molt_ord_at", 2);
                let result = self
                    .backend
                    .builder
                    .build_call(ord_at_fn, &[obj_bits.into(), index_bits.into()], "ord_at")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── StackAlloc: alloca for stack-resident slots ──
            // attrs: { "type": "i64" | "dynbox" | ... }
            // result: pointer stored as i64 (ptrtoint)
            OpCode::StackAlloc => {
                let i64_ty = self.backend.context.i64_type();
                let ptr = self
                    .backend
                    .builder
                    .build_alloca(i64_ty, "stack_slot")
                    .unwrap();
                let ptr_as_i64 = self
                    .backend
                    .builder
                    .build_ptr_to_int(ptr, i64_ty, "slot_ptr")
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, ptr_as_i64.into());
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── Free: stack-allocated slots are freed automatically — no-op ──
            OpCode::Free => {
                // Stack memory is reclaimed by the function epilogue; nothing to emit.
            }

            // ── BuildList: [item0, item1, ...] ──
            // Strategy: list_builder_new(capacity) + append + finish.
            OpCode::BuildList => {
                let i64_ty = self.backend.context.i64_type();
                let n = op.operands.len() as u64;
                let list_new_fn = self
                    .backend
                    .module
                    .get_function("molt_list_builder_new")
                    .unwrap();
                let builder = self
                    .backend
                    .builder
                    .build_call(list_new_fn, &[i64_ty.const_int(n, false).into()], "list")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self
                    .backend
                    .module
                    .get_function("molt_list_builder_append")
                    .unwrap();
                for &item_id in &op.operands {
                    let item_i64 = self.materialize_dynbox_operand(item_id);
                    self.backend
                        .builder
                        .build_call(push_fn, &[builder.into(), item_i64.into()], "list_push")
                        .unwrap();
                }
                let finish_fn = self
                    .backend
                    .module
                    .get_function("molt_list_builder_finish")
                    .unwrap();
                let list = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "list_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, list);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── BuildDict: {k0: v0, k1: v1, ...} ──
            // operands: [k0, v0, k1, v1, ...]  (pairs)
            OpCode::BuildDict => {
                let i64_ty = self.backend.context.i64_type();
                let n_pairs = (op.operands.len() / 2) as u64;
                let dict_new_fn = self
                    .backend
                    .module
                    .get_function("molt_dict_builder_new")
                    .unwrap();
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        dict_new_fn,
                        &[i64_ty.const_int(n_pairs, false).into()],
                        "dict_builder",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let dict_set_fn = self
                    .backend
                    .module
                    .get_function("molt_dict_builder_append")
                    .unwrap();
                let mut i = 0;
                while i + 1 < op.operands.len() {
                    let k_i64 = self.materialize_dynbox_operand(op.operands[i]);
                    let v_i64 = self.materialize_dynbox_operand(op.operands[i + 1]);
                    self.backend
                        .builder
                        .build_call(
                            dict_set_fn,
                            &[builder.into(), k_i64.into(), v_i64.into()],
                            "dict_append",
                        )
                        .unwrap();
                    i += 2;
                }
                let finish_fn = self
                    .backend
                    .module
                    .get_function("molt_dict_builder_finish")
                    .unwrap();
                let dict = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "dict_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, dict);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── BuildTuple: (item0, item1, ...) ──
            OpCode::BuildTuple => {
                let i64_ty = self.backend.context.i64_type();
                let n = op.operands.len() as u64;
                let tuple_builder_new = self
                    .backend
                    .module
                    .get_function("molt_list_builder_new")
                    .unwrap();
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        tuple_builder_new,
                        &[i64_ty.const_int(n, false).into()],
                        "tuple_builder",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self
                    .backend
                    .module
                    .get_function("molt_list_builder_append")
                    .unwrap();
                for &item_id in &op.operands {
                    let item_i64 = self.materialize_dynbox_operand(item_id);
                    self.backend
                        .builder
                        .build_call(push_fn, &[builder.into(), item_i64.into()], "tup_push")
                        .unwrap();
                }
                let finish_fn = self
                    .backend
                    .module
                    .get_function("molt_tuple_builder_finish")
                    .unwrap();
                let tup = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "tuple_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, tup);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── BuildSet: {item0, item1, ...} ──
            OpCode::BuildSet => {
                let i64_ty = self.backend.context.i64_type();
                let n = op.operands.len() as u64;
                let set_new_fn = self
                    .backend
                    .module
                    .get_function("molt_set_builder_new")
                    .unwrap();
                let builder = self
                    .backend
                    .builder
                    .build_call(
                        set_new_fn,
                        &[i64_ty.const_int(n, false).into()],
                        "set_builder",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let push_fn = self
                    .backend
                    .module
                    .get_function("molt_set_builder_append")
                    .unwrap();
                for &item_id in &op.operands {
                    let item_i64 = self.materialize_dynbox_operand(item_id);
                    self.backend
                        .builder
                        .build_call(push_fn, &[builder.into(), item_i64.into()], "set_append")
                        .unwrap();
                }
                let finish_fn = self
                    .backend
                    .module
                    .get_function("molt_set_builder_finish")
                    .unwrap();
                let set = self
                    .backend
                    .builder
                    .build_call(finish_fn, &[builder.into()], "set_finish")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, set);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── BuildSlice: slice(start, stop, step) ──
            // operands: [start, stop, step]   (already declared as molt_slice_new)
            OpCode::BuildSlice => {
                let i64_ty = self.backend.context.i64_type();
                let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                let none_val: BasicValueEnum<'ctx> = i64_ty.const_int(none_bits, false).into();

                let start = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v).into()
                } else {
                    none_val
                };
                let stop = if op.operands.len() > 1 {
                    let v = self.resolve(op.operands[1]);
                    self.ensure_i64(v).into()
                } else {
                    none_val
                };
                let step = if op.operands.len() > 2 {
                    let v = self.resolve(op.operands[2]);
                    self.ensure_i64(v).into()
                } else {
                    none_val
                };

                let slice_fn = self.backend.module.get_function("molt_slice_new").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(slice_fn, &[start.into(), stop.into(), step.into()], "slice")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── GetIter: iter(obj) ──
            OpCode::GetIter => {
                let obj = self.resolve(op.operands[0]);
                let obj_i64 = self.ensure_i64(obj);
                let get_iter_fn = self.backend.module.get_function("molt_get_iter").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(get_iter_fn, &[obj_i64.into()], "get_iter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── IterNext: next(iter) -> value (or StopIteration sentinel) ──
            OpCode::IterNext => {
                let iter = self.resolve(op.operands[0]);
                let iter_i64 = self.ensure_i64(iter);
                let iter_next_fn = self.backend.module.get_function("molt_iter_next").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(iter_next_fn, &[iter_i64.into()], "iter_next")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ForIter: advance iterator, returning next value or exhaustion sentinel ──
            OpCode::ForIter => {
                // Vectorization hint: when `vectorize = true` is set on this op (by the
                // vectorize analysis pass), the enclosing loop body is safe to vectorize.
                //
                // Per-loop vectorization metadata (`!{!"llvm.loop.vectorize.enable", i1 1}`)
                // requires attaching an MDNode to the loop back-edge branch instruction.
                // The inkwell API does not expose `LLVMSetMetadata` for branch instructions
                // nor the `MDNode`/`MDString` constructors needed to build loop metadata.
                // Vectorization is still enabled at the function level via `-march=native`
                // in the target machine (which enables +neon on ARM / +avx2 on x86), so
                // LLVM's loop vectorizer will analyze and vectorize eligible loops anyway.
                // To attach per-loop metadata, a raw `llvm-sys::LLVMSetMetadata` call on
                // the back-edge `BranchInst` would be needed.
                let _ = has_attr(op, "vectorize");

                let iter = self.resolve(op.operands[0]);
                let iter_i64 = self.ensure_i64(iter);
                let for_iter_fn = self.backend.module.get_function("molt_for_iter").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(for_iter_fn, &[iter_i64.into()], "for_iter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── Yield: suspend generator, yield value ──
            OpCode::AllocTask => {
                let result_id = op.results[0];
                let i64_ty = self.backend.context.i64_type();
                let closure_size = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let task_kind = op
                    .attrs
                    .get("task_kind")
                    .and_then(|v| match v {
                        AttrValue::Str(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .unwrap_or("future");
                let (kind_bits, payload_base) = match task_kind {
                    "generator" => (crate::TASK_KIND_GENERATOR, crate::GENERATOR_CONTROL_BYTES),
                    "future" => (crate::TASK_KIND_FUTURE, 0),
                    "coroutine" => (crate::TASK_KIND_COROUTINE, 0),
                    _ => panic!("unknown task kind: {task_kind}"),
                };
                let Some(poll_func_name) = op.attrs.get("s_value").and_then(|v| match v {
                    AttrValue::Str(s) => Some(s.as_str()),
                    _ => None,
                }) else {
                    panic!(
                        "alloc_task missing poll function name in {}",
                        self.func.name
                    );
                };
                let poll_fn = self.ensure_function_symbol(poll_func_name, 1, false);
                let poll_addr = self
                    .backend
                    .builder
                    .build_ptr_to_int(
                        poll_fn.as_global_value().as_pointer_value(),
                        i64_ty,
                        "task_poll_ptr",
                    )
                    .unwrap();
                let task_new_fn = self.ensure_runtime_i64_fn("molt_task_new", 3);
                let task_bits = self
                    .backend
                    .builder
                    .build_call(
                        task_new_fn,
                        &[
                            poll_addr.into(),
                            i64_ty.const_int(closure_size as u64, true).into(),
                            i64_ty.const_int(kind_bits as u64, true).into(),
                        ],
                        "task_new",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let ptr_ty = self
                    .backend
                    .context
                    .ptr_type(inkwell::AddressSpace::default());
                // `molt_task_new` returns the NaN-boxed task handle (QNAN | TAG_PTR
                // in the top 16 bits). The frame-payload stores below address raw
                // heap memory, so the boxing tag MUST be stripped first — mirroring
                // the native backend's `unbox_ptr_value(obj)` (simple_backend.rs)
                // before its `store` to the frame. Using the boxed bits directly as
                // a base address writes through `0x7FFC…`-tagged garbage → SIGSEGV
                // at generator creation. The boxed `task_bits` is still what flows
                // into the result value; only the field base pointer is unboxed.
                let task_ptr_bits = self.unbox_ptr_bits(self.ensure_i64(task_bits));
                let task_ptr = self
                    .backend
                    .builder
                    .build_int_to_ptr(task_ptr_bits, ptr_ty, "task_obj_ptr")
                    .unwrap();
                for (idx, &arg_id) in op.operands.iter().enumerate() {
                    let arg_bits = self.materialize_dynbox_operand(arg_id);
                    let field_ptr = unsafe {
                        self.backend
                            .builder
                            .build_gep(
                                i64_ty,
                                task_ptr,
                                &[i64_ty
                                    .const_int(((payload_base / 8) as usize + idx) as u64, false)],
                                &format!("task_payload_ptr_{idx}"),
                            )
                            .unwrap()
                    };
                    self.backend
                        .builder
                        .build_store(field_ptr, arg_bits)
                        .unwrap();
                    let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
                    let _ = self
                        .backend
                        .builder
                        .build_call(inc_fn, &[arg_bits.into()], "task_payload_inc_ref")
                        .unwrap();
                }
                self.values.insert(result_id, task_bits);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::StateSwitch => {
                // `state_switch` is now lowered as the first-class `StateDispatch`
                // terminator (see `lower_terminator`), never as a body op: the
                // SimpleIR `state_switch` op is structural (excluded from TIR
                // block ops by `gather_defs_uses`) and the SSA terminator builder
                // emits `Terminator::StateDispatch` for the dispatch block.
                // Reaching it here as a body op means the structural-op invariant
                // broke upstream — fail loud rather than emit a second (synthetic)
                // dispatch that double-switches the saved state.
                panic!(
                    "OpCode::StateSwitch reached the LLVM op-lowering body in '{}'; \
                     state_switch must lower as the StateDispatch terminator",
                    self.func.name
                );
            }
            OpCode::ClosureLoad => {
                let result_id = op.results[0];
                let self_bits = self.materialize_dynbox_operand(op.operands[0]);
                let offset = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let load_fn = self.ensure_runtime_i64_fn("molt_closure_load", 2);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        load_fn,
                        &[
                            self_bits.into(),
                            self.backend
                                .context
                                .i64_type()
                                .const_int(offset as u64, true)
                                .into(),
                        ],
                        "closure_load",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }
            OpCode::ClosureStore => {
                let self_bits = self.materialize_dynbox_operand(op.operands[0]);
                let val_bits = self.materialize_dynbox_operand(op.operands[1]);
                let offset = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let store_fn = self.ensure_runtime_i64_fn("molt_closure_store", 3);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        store_fn,
                        &[
                            self_bits.into(),
                            self.backend
                                .context
                                .i64_type()
                                .const_int(offset as u64, true)
                                .into(),
                            val_bits.into(),
                        ],
                        "closure_store",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::StateYield => {
                let next_state_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let self_bits = self.generator_self_bits();
                let pair_bits = self.materialize_dynbox_operand(op.operands[0]);
                let set_state_fn = self.ensure_runtime_void_fn("molt_obj_set_state", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[
                            self_bits.into(),
                            self.backend
                                .context
                                .i64_type()
                                .const_int(next_state_id as u64, true)
                                .into(),
                        ],
                        "state_yield_set_state",
                    )
                    .unwrap();
                let inc_fn = self.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
                let _ = self
                    .backend
                    .builder
                    .build_call(inc_fn, &[pair_bits.into()], "state_yield_inc_ref")
                    .unwrap();
                // The suspend `ret`s the yielded pair.  This `build_return`
                // terminates the suspend block; the main lowering loop detects
                // the terminator and moves on to the NEXT TIR block (the real
                // post-yield resume continuation, which the `StateDispatch`
                // terminator dispatches to).  We do NOT `position_at_end` into a
                // synthetic resume block — the continuation is a first-class TIR
                // block reached via the dispatch, and its phis were placed by the
                // SSA pass on the real `state_resume_edges`.
                self.backend.builder.build_return(Some(&pair_bits)).unwrap();
                let _ = next_state_id;
            }
            OpCode::StateTransition => {
                let (slot_id, pending_state_operand) = match op.operands.as_slice() {
                    [_, pending_state] => (None, *pending_state),
                    [_, slot, pending_state] => (Some(*slot), *pending_state),
                    other => panic!(
                        "state_transition expected 2 or 3 operands in {}: {:?}",
                        self.func.name, other
                    ),
                };
                let pending_state_id = self.const_i64_operand(pending_state_operand);
                let next_state_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let pending_bb = self.resume_block_for_state(pending_state_id);
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("state_transition must be inside a block");
                if current_bb != pending_bb {
                    self.record_llvm_edge(current_bb, pending_bb);
                    self.backend
                        .builder
                        .build_unconditional_branch(pending_bb)
                        .unwrap();
                    self.backend.builder.position_at_end(pending_bb);
                }
                let i64_ty = self.backend.context.i64_type();
                let self_bits = self.generator_self_bits();
                let future_bits = self.materialize_dynbox_operand(op.operands[0]);
                let pending_state_bits = i64_ty.const_int(pending_state_id as u64, true);
                let set_state_fn = self.ensure_runtime_void_fn("molt_obj_set_state", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[self_bits.into(), pending_state_bits.into()],
                        "state_transition_set_pending",
                    )
                    .unwrap();
                let poll_fn = self.ensure_runtime_i64_fn("molt_future_poll", 1);
                let res = self
                    .backend
                    .builder
                    .build_call(poll_fn, &[future_bits.into()], "state_transition_poll")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();
                let pending_const = i64_ty.const_int(crate::pending_bits() as u64, true);
                let is_pending = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::EQ,
                        res,
                        pending_const,
                        "state_transition_is_pending",
                    )
                    .unwrap();
                let pending_path = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("state_transition_pending{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                let ready_path = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("state_transition_ready{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                self.all_llvm_blocks.push(pending_path);
                self.all_llvm_blocks.push(ready_path);
                let branch_from_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("state_transition branch must be in block");
                self.record_llvm_edge(branch_from_bb, pending_path);
                self.record_llvm_edge(branch_from_bb, ready_path);
                self.backend
                    .builder
                    .build_conditional_branch(is_pending, pending_path, ready_path)
                    .unwrap();
                self.backend.builder.position_at_end(pending_path);
                let sleep_fn = self.ensure_runtime_i64_fn("molt_sleep_register", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        sleep_fn,
                        &[self_bits.into(), future_bits.into()],
                        "state_transition_sleep",
                    )
                    .unwrap();
                self.backend
                    .builder
                    .build_return(Some(&pending_const))
                    .unwrap();
                self.backend.builder.position_at_end(ready_path);
                if let Some(slot_id) = slot_id {
                    let slot_bits = self.raw_i64_operand(slot_id, ready_path);
                    let store_fn = self.ensure_runtime_i64_fn("molt_closure_store", 3);
                    let _ = self
                        .backend
                        .builder
                        .build_call(
                            store_fn,
                            &[self_bits.into(), slot_bits.into(), res.into()],
                            "state_transition_store",
                        )
                        .unwrap();
                } else if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, res.into());
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                let next_state_bits = i64_ty.const_int(next_state_id as u64, true);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[self_bits.into(), next_state_bits.into()],
                        "state_transition_set_next",
                    )
                    .unwrap();
                let next_bb = self.resume_block_for_state(next_state_id);
                let ready_from_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("state_transition ready must be in block");
                self.record_llvm_edge(ready_from_bb, next_bb);
                self.backend
                    .builder
                    .build_unconditional_branch(next_bb)
                    .unwrap();
                self.backend.builder.position_at_end(next_bb);
            }
            OpCode::ChanSendYield => {
                let pending_state_id = self.const_i64_operand(op.operands[2]);
                let next_state_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let pending_bb = self.resume_block_for_state(pending_state_id);
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("chan_send_yield must be inside a block");
                if current_bb != pending_bb {
                    self.record_llvm_edge(current_bb, pending_bb);
                    self.backend
                        .builder
                        .build_unconditional_branch(pending_bb)
                        .unwrap();
                    self.backend.builder.position_at_end(pending_bb);
                }
                let i64_ty = self.backend.context.i64_type();
                let self_bits = self.generator_self_bits();
                let chan_bits = self.materialize_dynbox_operand(op.operands[0]);
                let val_bits = self.materialize_dynbox_operand(op.operands[1]);
                let pending_state_bits = i64_ty.const_int(pending_state_id as u64, true);
                let set_state_fn = self.ensure_runtime_void_fn("molt_obj_set_state", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[self_bits.into(), pending_state_bits.into()],
                        "chan_send_set_pending",
                    )
                    .unwrap();
                let send_fn = self.ensure_runtime_i64_fn("molt_chan_send", 2);
                let res = self
                    .backend
                    .builder
                    .build_call(send_fn, &[chan_bits.into(), val_bits.into()], "chan_send")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();
                let pending_const = i64_ty.const_int(crate::pending_bits() as u64, true);
                let is_pending = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::EQ,
                        res,
                        pending_const,
                        "chan_send_is_pending",
                    )
                    .unwrap();
                let pending_path = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("chan_send_pending{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                let ready_path = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("chan_send_ready{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                self.all_llvm_blocks.push(pending_path);
                self.all_llvm_blocks.push(ready_path);
                let branch_from_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("chan_send_yield branch must be in block");
                self.record_llvm_edge(branch_from_bb, pending_path);
                self.record_llvm_edge(branch_from_bb, ready_path);
                self.backend
                    .builder
                    .build_conditional_branch(is_pending, pending_path, ready_path)
                    .unwrap();
                self.backend.builder.position_at_end(pending_path);
                self.backend
                    .builder
                    .build_return(Some(&pending_const))
                    .unwrap();
                self.backend.builder.position_at_end(ready_path);
                let next_state_bits = i64_ty.const_int(next_state_id as u64, true);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[self_bits.into(), next_state_bits.into()],
                        "chan_send_set_next",
                    )
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, res.into());
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                let next_bb = self.resume_block_for_state(next_state_id);
                self.record_llvm_edge(ready_path, next_bb);
                self.backend
                    .builder
                    .build_unconditional_branch(next_bb)
                    .unwrap();
                self.backend.builder.position_at_end(next_bb);
            }
            OpCode::ChanRecvYield => {
                let pending_state_id = self.const_i64_operand(op.operands[1]);
                let next_state_id = op
                    .attrs
                    .get("value")
                    .and_then(|v| match v {
                        AttrValue::Int(v) => Some(*v),
                        _ => None,
                    })
                    .unwrap_or(0);
                let pending_bb = self.resume_block_for_state(pending_state_id);
                let current_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("chan_recv_yield must be inside a block");
                if current_bb != pending_bb {
                    self.record_llvm_edge(current_bb, pending_bb);
                    self.backend
                        .builder
                        .build_unconditional_branch(pending_bb)
                        .unwrap();
                    self.backend.builder.position_at_end(pending_bb);
                }
                let i64_ty = self.backend.context.i64_type();
                let self_bits = self.generator_self_bits();
                let chan_bits = self.materialize_dynbox_operand(op.operands[0]);
                let pending_state_bits = i64_ty.const_int(pending_state_id as u64, true);
                let set_state_fn = self.ensure_runtime_void_fn("molt_obj_set_state", 2);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[self_bits.into(), pending_state_bits.into()],
                        "chan_recv_set_pending",
                    )
                    .unwrap();
                let recv_fn = self.ensure_runtime_i64_fn("molt_chan_recv", 1);
                let res = self
                    .backend
                    .builder
                    .build_call(recv_fn, &[chan_bits.into()], "chan_recv")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
                    .into_int_value();
                let pending_const = i64_ty.const_int(crate::pending_bits() as u64, true);
                let is_pending = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::EQ,
                        res,
                        pending_const,
                        "chan_recv_is_pending",
                    )
                    .unwrap();
                let pending_path = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("chan_recv_pending{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                let ready_path = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("chan_recv_ready{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                self.all_llvm_blocks.push(pending_path);
                self.all_llvm_blocks.push(ready_path);
                let branch_from_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("chan_recv_yield branch must be in block");
                self.record_llvm_edge(branch_from_bb, pending_path);
                self.record_llvm_edge(branch_from_bb, ready_path);
                self.backend
                    .builder
                    .build_conditional_branch(is_pending, pending_path, ready_path)
                    .unwrap();
                self.backend.builder.position_at_end(pending_path);
                self.backend
                    .builder
                    .build_return(Some(&pending_const))
                    .unwrap();
                self.backend.builder.position_at_end(ready_path);
                let next_state_bits = i64_ty.const_int(next_state_id as u64, true);
                let _ = self
                    .backend
                    .builder
                    .build_call(
                        set_state_fn,
                        &[self_bits.into(), next_state_bits.into()],
                        "chan_recv_set_next",
                    )
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, res.into());
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                let next_bb = self.resume_block_for_state(next_state_id);
                self.record_llvm_edge(ready_path, next_bb);
                self.backend
                    .builder
                    .build_unconditional_branch(next_bb)
                    .unwrap();
                self.backend.builder.position_at_end(next_bb);
            }
            // ── Yield: suspend generator, yield value ──
            OpCode::Yield => {
                let val = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    // yield without value yields None
                    let none_bits = nanbox::QNAN | nanbox::TAG_NONE;
                    self.backend.context.i64_type().const_int(none_bits, false)
                };
                let yield_fn = self.backend.module.get_function("molt_yield").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(yield_fn, &[val.into()], "yield")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── YieldFrom: delegate to sub-generator ──
            OpCode::YieldFrom => {
                let subiter = self.resolve(op.operands[0]);
                let subiter_i64 = self.ensure_i64(subiter);
                let yield_from_fn = self.backend.module.get_function("molt_yield_from").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(yield_from_fn, &[subiter_i64.into()], "yield_from")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── Raise: raise exception ──
            OpCode::Raise => {
                let exc = self.resolve(op.operands[0]);
                let exc_i64 = self.ensure_i64(exc);
                let raise_fn = self.backend.module.get_function("molt_raise").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(raise_fn, &[exc_i64.into()], "raise")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if !op.results.is_empty() {
                    self.values.insert(op.results[0], result);
                    self.value_types.insert(op.results[0], TirType::DynBox);
                }
            }

            // ── WarnStderr: side-effecting diagnostic emit ──
            OpCode::WarnStderr => {
                let msg = self.resolve(op.operands[0]);
                let msg_i64 = self.ensure_i64(msg);
                let warn_fn = self
                    .backend
                    .module
                    .get_function("molt_warn_stderr")
                    .unwrap();
                self.backend
                    .builder
                    .build_call(warn_fn, &[msg_i64.into()], "warn_stderr")
                    .unwrap();
                if let Some(&result_id) = op.results.first() {
                    let none_val: BasicValueEnum<'ctx> = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                        .into();
                    self.values.insert(result_id, none_val);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ExceptionPending: read the runtime exception-pending flag as
            //    a raw i64 boolean (`molt_exception_pending() != 0`).  Produced
            //    by `loop_break_if_exception` and consumed as the condition of
            //    the loop-exit CondBranch that breaks an iterator-consumer loop
            //    on a mid-iteration raise.  Non-foldable: it observes mutable
            //    runtime state, so the value (and the break) always survive.
            OpCode::ExceptionPending => {
                let pend_fn = self
                    .backend
                    .module
                    .get_function("molt_exception_pending")
                    .unwrap_or_else(|| {
                        let i64_ty = self.backend.context.i64_type();
                        let fn_ty = i64_ty.fn_type(&[], false);
                        self.backend.module.add_function(
                            "molt_exception_pending",
                            fn_ty,
                            Some(inkwell::module::Linkage::External),
                        )
                    });
                let raw = self
                    .backend
                    .builder
                    .build_call(pend_fn, &[], "exc_pending")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, raw);
                    // `molt_exception_pending` returns a raw i64 0/1 (NOT a
                    // NaN-boxed bool), so the consuming CondBranch must test it
                    // with `!= 0` (the TirType::I64 path) rather than routing it
                    // through `molt_is_truthy`, which would misinterpret the bit
                    // pattern of `1` as a boxed value.
                    self.value_types.insert(result_id, TirType::I64);
                }
            }

            // ── FunctionDefaultsVersion: read a function object's
            //    __defaults__/__kwdefaults__ mutation version stamp as a boxed
            //    inline int (`molt_function_defaults_version(func)`).  Produced
            //    by the compile-time defaults-devirt deopt guard and consumed by
            //    its `== 0` compare (baked literal vs live read).  Non-foldable:
            //    it observes mutable runtime state, so the read always survives.
            OpCode::FunctionDefaultsVersion => {
                let ver_fn = self.ensure_runtime_i64_fn("molt_function_defaults_version", 1);
                let func_val = op
                    .operands
                    .first()
                    .and_then(|id| self.values.get(id).copied())
                    .expect("FunctionDefaultsVersion operand not materialized");
                let raw = self
                    .backend
                    .builder
                    .build_call(ver_fn, &[func_val.into()], "func_defaults_version")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, raw);
                    // Returns a NaN-boxed inline int; the consuming `== 0`
                    // compare routes through the boxed-int equality path.
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── CheckException: inspect the current exception state ──
            OpCode::CheckException => {
                let check_fn = self
                    .backend
                    .module
                    .get_function("molt_exception_pending")
                    .unwrap_or_else(|| {
                        let i64_ty = self.backend.context.i64_type();
                        let fn_ty = i64_ty.fn_type(&[], false);
                        self.backend.module.add_function(
                            "molt_exception_pending",
                            fn_ty,
                            Some(inkwell::module::Linkage::External),
                        )
                    });
                let result = self
                    .backend
                    .builder
                    .build_call(check_fn, &[], "check_exc")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
                let Some(target_label) = op.attrs.get("value").and_then(|v| match v {
                    AttrValue::Int(v) => Some(*v),
                    _ => None,
                }) else {
                    return;
                };
                let Some(target_block_id) = self
                    .func
                    .label_id_map
                    .iter()
                    .find_map(|(bid, label)| (*label == target_label).then_some(BlockId(*bid)))
                else {
                    self.record_fatal(format!(
                        "check_exception target label {} is not present in label map",
                        target_label
                    ));
                    return;
                };
                let Some(&target_bb) = self.block_map.get(&target_block_id) else {
                    self.record_fatal(format!(
                        "check_exception target block {:?} is not present in LLVM block map",
                        target_block_id
                    ));
                    return;
                };
                let continue_bb = self.backend.context.append_basic_block(
                    self.llvm_fn,
                    &format!("check_exc_cont{}", self.synthetic_block_counter),
                );
                self.synthetic_block_counter += 1;
                self.all_llvm_blocks.push(continue_bb);
                let pending = self.ensure_i64(result);
                let has_exception = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        pending,
                        self.backend.context.i64_type().const_zero(),
                        "check_exc_pending",
                    )
                    .unwrap();
                let branch_from_bb = self
                    .backend
                    .builder
                    .get_insert_block()
                    .expect("must be inside a block");
                self.record_branch_args(
                    source_block,
                    branch_from_bb,
                    target_block_id,
                    "check-exception-edge",
                    &op.operands,
                );
                self.record_llvm_edge(branch_from_bb, target_bb);
                self.record_llvm_edge(branch_from_bb, continue_bb);
                self.backend
                    .builder
                    .build_conditional_branch(has_exception, target_bb, continue_bb)
                    .unwrap();
                self.backend.builder.position_at_end(continue_bb);
            }

            // ── Import: import module by name ──
            OpCode::Import => {
                let result_id = op.results[0];
                let name = if let Some(&name_id) = op.operands.first() {
                    self.resolve(name_id)
                } else if let Some(AttrValue::Str(module_name)) = op.attrs.get("module") {
                    self.intern_string_const(module_name)
                } else if let Some(AttrValue::Str(module_name)) = op.attrs.get("s_value") {
                    self.intern_string_const(module_name)
                } else if let Some(AttrValue::Str(module_name)) = op.attrs.get("_var") {
                    self.intern_string_const(module_name)
                } else {
                    panic!(
                        "Import op missing module operand/attr in {}",
                        self.func.name
                    );
                };
                let name_i64 = self.ensure_i64(name);
                let import_fn = self
                    .backend
                    .module
                    .get_function("molt_module_import")
                    .unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(import_fn, &[name_i64.into()], "import")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }

            // ── ImportFrom: from module import name ──
            // operands: [module, attr_name]
            OpCode::ImportFrom => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_get_attr",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleCacheGet: module-cache lookup by name ──
            // operands: [module_name]
            OpCode::ModuleCacheGet => {
                let result_id = op.results[0];
                let get_fn = self.ensure_runtime_i64_fn("molt_module_cache_get", 1);
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(get_fn, &[name_bits.into()], "module_cache_get")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.values.insert(result_id, result);
                self.value_types.insert(result_id, TirType::DynBox);
            }

            // ── ModuleCacheSet: module-cache mutation by name ──
            // operands: [module_name, module]
            OpCode::ModuleCacheSet => {
                let set_fn = self.ensure_runtime_i64_fn("molt_module_cache_set", 2);
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let module_bits = self.materialize_dynbox_operand(op.operands[1]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[name_bits.into(), module_bits.into()],
                        "module_cache_set",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleCacheDel: module-cache deletion by name ──
            // operands: [module_name]
            OpCode::ModuleCacheDel => {
                let del_fn = self.ensure_runtime_i64_fn("molt_module_cache_del", 1);
                let name_bits = self.materialize_dynbox_operand(op.operands[0]);
                let result = self
                    .backend
                    .builder
                    .build_call(del_fn, &[name_bits.into()], "module_cache_del")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleGetAttr: module attribute read ──
            // operands: [module, attr_name]
            OpCode::ModuleGetAttr => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_get_attr",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleImportFrom: `from M import name` binding ──
            // operands: [module, attr_name]. CPython IMPORT_FROM semantics:
            // ImportError (not AttributeError) on miss, with a sys.modules
            // submodule fallback (see molt_module_import_from).
            OpCode::ModuleImportFrom => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_import_from",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleGetGlobal: CPython-style module global lookup ──
            // operands: [module, global_name]
            OpCode::ModuleGetGlobal => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_get_global",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleGetName: module name/attribute lookup helper ──
            // operands: [module, attr_name]
            OpCode::ModuleGetName => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_get_name",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleSetAttr: module attribute mutation ──
            // operands: [module, attr_name, value]
            OpCode::ModuleSetAttr => {
                let set_fn = self.ensure_runtime_i64_fn("molt_module_set_attr", 3);
                let module_bits = self.materialize_dynbox_operand(op.operands[0]);
                let attr_bits = self.materialize_dynbox_operand(op.operands[1]);
                let val_bits = self.materialize_dynbox_operand(op.operands[2]);
                let result = self
                    .backend
                    .builder
                    .build_call(
                        set_fn,
                        &[module_bits.into(), attr_bits.into(), val_bits.into()],
                        "module_set_attr",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── ModuleDelGlobal: CPython-style module global deletion ──
            // operands: [module, global_name]
            OpCode::ModuleDelGlobal => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_del_global",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::ModuleDelGlobalIfPresent => {
                let result = self.call_runtime_2_boxed(
                    "molt_module_del_global_if_present",
                    op.operands[0],
                    op.operands[1],
                );
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── SCF dialect ops ──
            // Structured control flow ops are desugared into LLVM basic blocks.
            // ScfIf uses conditional branches to then/else blocks with a merge phi.
            // ScfFor/ScfWhile delegate to runtime helpers since full loop lowering
            // requires loop analysis infrastructure (induction variable detection,
            // trip count computation) that lives in a separate pass.
            // ScfYield maps to a runtime call that returns its value.
            OpCode::ScfIf => {
                let _ = has_attr(op, "vectorize");
                let i64_ty = self.backend.context.i64_type();

                // Resolve condition and coerce to i1.
                let cond_i64 = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let truthy_fn = self.backend.module.get_function("molt_is_truthy").unwrap();
                let truthy_result = self
                    .backend
                    .builder
                    .build_call(truthy_fn, &[cond_i64.into()], "scf_if_truthy")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let cond_i1 = self
                    .backend
                    .builder
                    .build_int_compare(
                        inkwell::IntPredicate::NE,
                        truthy_result.into_int_value(),
                        i64_ty.const_int(0, false),
                        "scf_if_cond",
                    )
                    .unwrap();

                // Resolve then/else function operands.
                let then_fn_bits = if op.operands.len() > 1 {
                    let v = self.resolve(op.operands[1]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let else_fn_bits = if op.operands.len() > 2 {
                    let v = self.resolve(op.operands[2]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };

                // Create basic blocks for then, else, and merge.
                let current_fn = self.llvm_fn;
                let then_bb = self
                    .backend
                    .context
                    .append_basic_block(current_fn, "scf_if_then");
                let else_bb = self
                    .backend
                    .context
                    .append_basic_block(current_fn, "scf_if_else");
                let merge_bb = self
                    .backend
                    .context
                    .append_basic_block(current_fn, "scf_if_merge");
                self.all_llvm_blocks.push(then_bb);
                self.all_llvm_blocks.push(else_bb);
                self.all_llvm_blocks.push(merge_bb);

                self.backend
                    .builder
                    .build_conditional_branch(cond_i1, then_bb, else_bb)
                    .unwrap();

                // Then block: call then_fn via molt_call_0 and branch to merge.
                self.backend.builder.position_at_end(then_bb);
                let call0_fn = self.backend.module.get_function("molt_call_0").unwrap();
                let then_result = self
                    .backend
                    .builder
                    .build_call(call0_fn, &[then_fn_bits.into()], "scf_then_result")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.backend
                    .builder
                    .build_unconditional_branch(merge_bb)
                    .unwrap();
                let then_exit_bb = self.backend.builder.get_insert_block().unwrap();

                // Else block: call else_fn via molt_call_0 and branch to merge.
                self.backend.builder.position_at_end(else_bb);
                let else_result = self
                    .backend
                    .builder
                    .build_call(call0_fn, &[else_fn_bits.into()], "scf_else_result")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                self.backend
                    .builder
                    .build_unconditional_branch(merge_bb)
                    .unwrap();
                let else_exit_bb = self.backend.builder.get_insert_block().unwrap();

                // Merge block: phi node selects then/else result.
                self.backend.builder.position_at_end(merge_bb);
                let phi = self
                    .backend
                    .builder
                    .build_phi(i64_ty, "scf_if_phi")
                    .unwrap();
                phi.add_incoming(&[(&then_result, then_exit_bb), (&else_result, else_exit_bb)]);
                let phi_val = phi.as_basic_value();

                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, phi_val);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::ScfFor => {
                // ScfFor delegates to the runtime: full loop lowering requires
                // induction variable detection and trip count analysis that runs
                // as a separate TIR pass before LLVM lowering.
                let _ = has_attr(op, "vectorize");
                let i64_ty = self.backend.context.i64_type();
                let lb = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let ub = if op.operands.len() > 1 {
                    let v = self.resolve(op.operands[1]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let step = if op.operands.len() > 2 {
                    let v = self.resolve(op.operands[2]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(1, false)
                };
                let body_fn_bits = if op.operands.len() > 3 {
                    let v = self.resolve(op.operands[3]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let scf_for_fn = self.backend.module.get_function("molt_scf_for").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(
                        scf_for_fn,
                        &[lb.into(), ub.into(), step.into(), body_fn_bits.into()],
                        "scf_for",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::ScfWhile => {
                // ScfWhile delegates to the runtime: full loop lowering requires
                // condition hoisting and break/continue analysis.
                let _ = has_attr(op, "vectorize");
                let i64_ty = self.backend.context.i64_type();
                let cond_fn_bits = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let body_fn_bits = if op.operands.len() > 1 {
                    let v = self.resolve(op.operands[1]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let scf_while_fn = self.backend.module.get_function("molt_scf_while").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(
                        scf_while_fn,
                        &[cond_fn_bits.into(), body_fn_bits.into()],
                        "scf_while",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::ScfYield => {
                // ScfYield returns its operand value (or None if no operand).
                let _ = has_attr(op, "vectorize");
                let i64_ty = self.backend.context.i64_type();
                let val = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(nanbox::QNAN | nanbox::TAG_NONE, false)
                };
                let scf_yield_fn = self.backend.module.get_function("molt_scf_yield").unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(scf_yield_fn, &[val.into()], "scf_yield")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // ── Deopt: transfer execution back to interpreter ──
            OpCode::Deopt => {
                let i64_ty = self.backend.context.i64_type();
                let frame_bits = if !op.operands.is_empty() {
                    let v = self.resolve(op.operands[0]);
                    self.ensure_i64(v)
                } else {
                    i64_ty.const_int(0, false)
                };
                let deopt_fn = self
                    .backend
                    .module
                    .get_function("molt_deopt_transfer")
                    .unwrap();
                let result = self
                    .backend
                    .builder
                    .build_call(deopt_fn, &[frame_bits.into()], "deopt")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }

            // Exception region markers. LLVM still uses polling-based Molt
            // exceptions, but the runtime expects a handler frame to be
            // established around try regions so raise/catch semantics match
            // native and wasm.
            OpCode::TryStart => {
                let enter_fn = self.ensure_runtime_i64_fn("molt_exception_stack_enter", 0);
                let baseline = self
                    .backend
                    .builder
                    .build_call(enter_fn, &[], "try_enter")
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let baseline_slot = self.build_entry_i64_alloca("try_baseline");
                self.backend
                    .builder
                    .build_store(baseline_slot, baseline)
                    .unwrap();
                self.try_stack_baselines.push(baseline_slot);
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, baseline);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::TryEnd => {
                if let Some(baseline_slot) = self.try_stack_baselines.pop() {
                    let exit_fn = self.ensure_runtime_i64_fn("molt_exception_stack_exit", 1);
                    let baseline_bits = self
                        .backend
                        .builder
                        .build_load(
                            self.backend.context.i64_type(),
                            baseline_slot,
                            "try_baseline_load",
                        )
                        .unwrap()
                        .into_int_value();
                    let result = self
                        .backend
                        .builder
                        .build_call(exit_fn, &[baseline_bits.into()], "try_exit")
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic();
                    if let Some(&result_id) = op.results.first() {
                        self.values.insert(result_id, result);
                        self.value_types.insert(result_id, TirType::DynBox);
                    }
                }
            }
            OpCode::IterNextUnboxed => {
                let iter_bits = self.materialize_dynbox_operand(op.operands[0]);
                let i64_ty = self.backend.context.i64_type();
                let val_ptr = self
                    .backend
                    .builder
                    .build_alloca(i64_ty, "iter_next_unboxed_value")
                    .unwrap();
                let val_ptr_bits = self
                    .backend
                    .builder
                    .build_ptr_to_int(val_ptr, i64_ty, "iter_next_unboxed_value_ptr")
                    .unwrap();
                let iter_next_fn = self.ensure_runtime_i64_fn("molt_iter_next_unboxed", 2);
                let done_bits = self
                    .backend
                    .builder
                    .build_call(
                        iter_next_fn,
                        &[iter_bits.into(), val_ptr_bits.into()],
                        "iter_next_unboxed",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic();
                let value_bits = self
                    .backend
                    .builder
                    .build_load(i64_ty, val_ptr, "iter_next_unboxed_value_load")
                    .unwrap();
                if let Some(&value_id) = op.results.first() {
                    self.values.insert(value_id, value_bits);
                    self.value_types.insert(value_id, TirType::DynBox);
                }
                if let Some(&done_id) = op.results.get(1) {
                    self.values.insert(done_id, done_bits);
                    self.value_types.insert(done_id, TirType::DynBox);
                }
            }
            OpCode::ObjectNewBound | OpCode::ObjectNewBoundStack => {
                let Some(&class_id) = op.operands.first() else {
                    panic!("{:?} requires class operand", op.opcode);
                };
                let class_bits = self.materialize_dynbox_operand(class_id);
                let result = if let Some(AttrValue::Int(payload_size)) = op.attrs.get("value")
                    && *payload_size > 0
                {
                    let new_fn = self.ensure_runtime_i64_fn("molt_object_new_bound_sized", 2);
                    let size_bits = self
                        .backend
                        .context
                        .i64_type()
                        .const_int(*payload_size as u64, false);
                    self.backend
                        .builder
                        .build_call(
                            new_fn,
                            &[class_bits.into(), size_bits.into()],
                            "object_new_bound_sized",
                        )
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic()
                } else {
                    let new_fn = self.ensure_runtime_i64_fn("molt_object_new_bound", 1);
                    self.backend
                        .builder
                        .build_call(new_fn, &[class_bits.into()], "object_new_bound")
                        .unwrap()
                        .try_as_basic_value()
                        .unwrap_basic()
                };
                if let Some(&result_id) = op.results.first() {
                    self.values.insert(result_id, result);
                    self.value_types.insert(result_id, TirType::DynBox);
                }
            }
            OpCode::StateBlockStart | OpCode::StateBlockEnd => {}
        }
    }
}
