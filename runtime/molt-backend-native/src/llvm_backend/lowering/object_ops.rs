use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
use molt_ir::OpKind;

impl<'ctx, 'func> FunctionLowering<'ctx, 'func> {
    pub(super) fn emit_load_attr(&mut self, op: &TirOp) {
        let result_id = op.results[0];
        let original_kind = op.origin;
        if matches!(original_kind, Some(OpKind::GetAttrName)) && op.operands.len() >= 2 {
            let get_fn = self.ensure_runtime_import(RuntimeFn::GetAttrName);
            let val = self.borrowed_runtime_call_value(
                get_fn,
                &[
                    RuntimeArg::Operand(op.operands[0]),
                    RuntimeArg::Operand(op.operands[1]),
                ],
                false,
                "get_attr_name",
                "get_attr_name_dyn",
            );
            self.values.set_value(result_id, val.into());
            self.values.set_repr(
                result_id,
                crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
            );
            return;
        }
        if matches!(original_kind, Some(OpKind::Load | OpKind::GuardedLoad))
            && !op.operands.is_empty()
        {
            self.emit_field_slot_get(op);
            return;
        }
        if matches!(original_kind, Some(OpKind::GuardedFieldGet)) {
            // operands = [obj, expected_version]; the class statement is the
            // op's `class_definition` (HF-220).
            assert_eq!(op.operands.len(), 2, "admitted guarded_field_get operands");
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
            let get_fn = self.ensure_runtime_import(RuntimeFn::GuardedFieldGet);
            let offset_bits = self
                .backend
                .context
                .i64_type()
                .const_int(offset as u64, true);
            let definition_slot = self.class_definition_slot(op);
            let val = self.borrowed_runtime_call_value(
                get_fn,
                &[
                    RuntimeArg::Operand(op.operands[0]),
                    RuntimeArg::Word(definition_slot.into()),
                    RuntimeArg::Operand(op.operands[1]),
                    RuntimeArg::Word(offset_bits.into()),
                    RuntimeArg::Word(attr_ptr_bits.into()),
                    RuntimeArg::Word(attr_len_bits.into()),
                ],
                false,
                "guarded_field_get",
                "guarded_field_get",
            );
            self.values.set_value(result_id, val.into());
            self.values.set_repr(
                result_id,
                crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
            );
            return;
        }
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
        let mut custody = self.begin_borrowed_operands(&op.operands[..1], "get_attr");
        let obj_bits = self.borrowed_operand(&mut custody, op.operands[0]);
        let site_kind = original_kind
            .filter(|kind| matches!(*kind, OpKind::GetAttrGenericObj | OpKind::GetAttrGenericPtr));
        let val = if let Some(site_kind) = site_kind {
            let runtime_name = RuntimeFn::GetAttrObjectIc;
            let get_fn = self.ensure_runtime_import(runtime_name);
            let (attr_ptr, attr_len_bits) = self.raw_string_const_ptr_and_len(attr_name);
            let site_bits = self.source_call_site_bits(op, site_kind.as_str());
            let call_args_generic = [
                obj_bits.into(),
                attr_ptr.into(),
                attr_len_bits.into(),
                site_bits.into(),
            ];
            self.backend
                .builder
                .build_execution_call(get_fn, &call_args_generic, runtime_name.symbol())
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_int_value()
        } else {
            let runtime_name = RuntimeFn::GetAttrName;
            let get_fn = self.ensure_runtime_import(runtime_name);
            self.with_owned_name(attr_name, |this, name_bits| {
                this.backend
                    .builder
                    .build_execution_call(
                        get_fn,
                        &[obj_bits.into(), name_bits.into()],
                        runtime_name.symbol(),
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
            })
            .into_int_value()
        };
        // Runtime getattr entry points return one owned result on every
        // successful path, including IC hits. Preserve that single authority;
        // a backend-side retain would leak bound-method receivers.
        let val = self.finish_borrowed_operands(custody, val, "get_attr_result", |_| {});
        self.values.set_value(result_id, val.into());
        self.values.set_repr(
            result_id,
            crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
        );
    }

    pub(super) fn emit_store_attr(&mut self, op: &TirOp) {
        let original_kind = op.origin;
        // Attribute setters borrow the receiver and value and return the
        // immortal None, so a discarded result needs no release.
        if matches!(original_kind, Some(OpKind::SetAttrName)) && op.operands.len() >= 3 {
            let set_fn = self.ensure_runtime_import(RuntimeFn::SetAttrName);
            self.emit_borrowed_runtime_call(
                op,
                set_fn,
                &[
                    RuntimeArg::Operand(op.operands[0]),
                    RuntimeArg::Operand(op.operands[1]),
                    RuntimeArg::Operand(op.operands[2]),
                ],
                ResultCustody::Unowned,
                "set_attr_name",
                "set_attr_name_dyn",
            );
            return;
        }
        if matches!(original_kind, Some(OpKind::Store)) && op.operands.len() >= 2 {
            self.emit_field_slot_set(op);
            return;
        }
        if matches!(original_kind, Some(OpKind::GuardedFieldSet)) {
            // operands = [obj, expected_version, value] (HF-220).
            assert_eq!(op.operands.len(), 3, "admitted guarded_field_set operands");
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
            let set_fn = self.ensure_runtime_import(RuntimeFn::GuardedFieldSet);
            let offset_bits = self
                .backend
                .context
                .i64_type()
                .const_int(offset as u64, true);
            let definition_slot = self.class_definition_slot(op);
            self.emit_borrowed_runtime_call(
                op,
                set_fn,
                &[
                    RuntimeArg::Operand(op.operands[0]),
                    RuntimeArg::Word(definition_slot.into()),
                    RuntimeArg::Operand(op.operands[1]),
                    RuntimeArg::Word(offset_bits.into()),
                    RuntimeArg::Operand(op.operands[2]),
                    RuntimeArg::Word(attr_ptr_bits.into()),
                    RuntimeArg::Word(attr_len_bits.into()),
                ],
                ResultCustody::Unowned,
                "guarded_field_set",
                "guarded_field_set",
            );
            return;
        }
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
        let mut custody = self.begin_borrowed_operands(&op.operands[..2], "set_attr");
        let obj_i64 = self.borrowed_operand(&mut custody, op.operands[0]);
        let val_i64 = self.borrowed_operand(&mut custody, op.operands[1]);
        let site_kind = original_kind
            .filter(|kind| matches!(*kind, OpKind::SetAttrGenericObj | OpKind::SetAttrGenericPtr));
        let result = if let Some(site_kind) = site_kind {
            // A generic write site carries its id, so a warm site writes the
            // inferred field without building the name string.
            let runtime_name = RuntimeFn::SetAttrObjectIc;
            let set_fn = self.ensure_runtime_import(runtime_name);
            let (attr_ptr, attr_len_bits) = self.raw_string_const_ptr_and_len(attr_name);
            let site_bits = self.source_call_site_bits(op, site_kind.as_str());
            self.backend
                .builder
                .build_execution_call(
                    set_fn,
                    &[
                        obj_i64.into(),
                        attr_ptr.into(),
                        attr_len_bits.into(),
                        val_i64.into(),
                        site_bits.into(),
                    ],
                    runtime_name.symbol(),
                )
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
                .into_int_value()
        } else {
            let set_fn = self.ensure_runtime_import(RuntimeFn::SetAttrName);
            self.with_owned_name(attr_name, |this, name_i64| {
                this.backend
                    .builder
                    .build_execution_call(
                        set_fn,
                        &[obj_i64.into(), name_i64.into(), val_i64.into()],
                        "setattr",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
            })
            .into_int_value()
        };
        let result = self.finish_borrowed_operands(custody, result, "set_attr_result", |_| {});
        if !op.results.is_empty() {
            self.values.set_value(op.results[0], result.into());
            self.values.set_repr(
                op.results[0],
                crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
            );
        }
    }

    pub(super) fn emit_del_attr(&mut self, op: &TirOp) {
        let original_kind = op.origin;
        // Attribute deletion borrows the receiver and returns the immortal None.
        if matches!(original_kind, Some(OpKind::DelAttrName)) && op.operands.len() >= 2 {
            let del_fn = self.ensure_runtime_import(RuntimeFn::DelAttrName);
            self.emit_borrowed_runtime_call(
                op,
                del_fn,
                &[
                    RuntimeArg::Operand(op.operands[0]),
                    RuntimeArg::Operand(op.operands[1]),
                ],
                ResultCustody::Unowned,
                "del_attr_name",
                "del_attr_name_dyn",
            );
            return;
        }
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
        let mut custody = self.begin_borrowed_operands(&op.operands[..1], "del_attr");
        let obj_bits = self.borrowed_operand(&mut custody, op.operands[0]);
        let del_fn = self.ensure_runtime_import(RuntimeFn::DelAttrName);
        let val = self
            .with_owned_name(attr_name, |this, name_bits| {
                this.backend
                    .builder
                    .build_execution_call(
                        del_fn,
                        &[obj_bits.into(), name_bits.into()],
                        "del_attr_name",
                    )
                    .unwrap()
                    .try_as_basic_value()
                    .unwrap_basic()
            })
            .into_int_value();
        let val = self.finish_borrowed_operands(custody, val, "del_attr_result", |_| {});
        if !op.results.is_empty() {
            self.values.set_value(op.results[0], val.into());
            self.values.set_repr(
                op.results[0],
                crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
            );
        }
    }

    pub(super) fn emit_index(&mut self, op: &TirOp) {
        let result_id = op.results[0];
        // BCE: when the bounds-check elimination pass has proven the index
        // is in-range, we call `molt_getitem_unchecked` which skips the
        // runtime bounds check and associated branch entirely.
        let symbol = if has_attr(op, "bce_safe") {
            RuntimeFn::GetitemUnchecked
        } else {
            RuntimeFn::GetitemMethod
        };
        // A container that may be a list or tuple takes the shared read, which
        // inlines an exact list or tuple at an inline int; its miss is the
        // runtime entry (`molt_codegen_abi::list_item`).
        let mut args = vec![
            RuntimeArg::Operand(op.operands[0]),
            RuntimeArg::Operand(op.operands[1]),
        ];
        let get_fn = if let Some(containers) = self.item_read_containers(op.operands[0]) {
            args.push(RuntimeArg::Word(self.list_item_admission().into()));
            self.ensure_list_item_get_helper(containers, symbol)
        } else {
            self.ensure_runtime_import(symbol)
        };
        let val = self.borrowed_runtime_call_value(get_fn, &args, false, "index", symbol.symbol());
        self.values.set_value(result_id, val.into());
        self.values.set_repr(
            result_id,
            crate::codegen_ssa::HeldCarrier::of(&TirType::DynBox),
        );
    }

    /// Item assignment and deletion borrow their operands; the container
    /// retains what it stores, and the runtime returns only None or a boolean.
    pub(super) fn emit_store_index(&mut self, op: &TirOp) {
        // A container that may be a list takes the shared list store; its
        // miss is `molt_setitem_method`.
        let mut args = vec![
            RuntimeArg::Operand(op.operands[0]),
            RuntimeArg::Operand(op.operands[1]),
            RuntimeArg::Operand(op.operands[2]),
        ];
        let set_fn = if self.container_may_be_list(op.operands[0]) {
            args.push(RuntimeArg::Word(self.list_item_admission().into()));
            self.ensure_list_item_set_helper()
        } else {
            self.ensure_runtime_import(RuntimeFn::SetitemMethod)
        };
        self.emit_borrowed_runtime_call(
            op,
            set_fn,
            &args,
            ResultCustody::Unowned,
            "store_index",
            "setitem",
        );
    }

    pub(super) fn emit_del_index(&mut self, op: &TirOp) {
        let del_fn = self.ensure_runtime_import(RuntimeFn::DelitemMethod);
        self.emit_borrowed_runtime_call(
            op,
            del_fn,
            &[
                RuntimeArg::Operand(op.operands[0]),
                RuntimeArg::Operand(op.operands[1]),
            ],
            ResultCustody::Unowned,
            "del_index",
            RuntimeFn::DelitemMethod.symbol(),
        );
    }
}
