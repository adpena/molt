use super::*;

#[test]
fn guarded_fields_preserve_tagged_receivers_at_runtime_admission() {
    for kind in ["guarded_field_get", "guarded_field_set"] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let read = kind == "guarded_field_get";
        let mut params = vec![TirType::DynBox; if read { 3 } else { 4 }];
        // An unchecked annotation may supply this hint for an actual scalar.
        params[0] = TirType::UserClass("C".into());
        let mut func = TirFunction::new(
            kind.into(),
            params,
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: if read {
                OpCode::LoadAttr
            } else {
                OpCode::StoreAttr
            },
            operands: (0..if read { 3 } else { 4 }).map(ValueId).collect(),
            results: if read { vec![result] } else { vec![] },
            attrs: AttrDict::from([
                ("_original_kind".into(), AttrValue::Str(kind.into())),
                ("name".into(), AttrValue::Str("x".into())),
                ("value".into(), AttrValue::Int(0)),
            ]),
            source_span: None,
        });
        if !read {
            entry.ops.push(const_none_def(result));
        }
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        backend
            .module
            .verify()
            .expect("tagged guarded field ABI must verify");
        let ir = llvm_fn.print_to_string().to_string();
        assert!(
            ir.contains(&format!("@molt_{kind}(i64 %0, i64 %1, i64 %2")),
            "{ir}"
        );
        assert!(
            !ir.contains("inttoptr") && !ir.contains("ptr_unbox"),
            "{ir}"
        );
    }
}

#[test]
fn typed_field_inline_access_checks_receiver_before_dereferencing() {
    for kind in ["load", "store"] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let is_load = kind == "load";
        let mut func = TirFunction::new(
            format!("field_{kind}"),
            vec![TirType::DynBox; if is_load { 1 } else { 2 }],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: if is_load {
                OpCode::LoadAttr
            } else {
                OpCode::StoreAttr
            },
            operands: if is_load {
                vec![ValueId(0)]
            } else {
                vec![ValueId(0), ValueId(1)]
            },
            results: if is_load { vec![result] } else { vec![] },
            attrs: AttrDict::from([
                ("_original_kind".into(), AttrValue::Str(kind.into())),
                ("value".into(), AttrValue::Int(0)),
            ]),
            source_span: None,
        });
        if !is_load {
            entry.ops.push(const_none_def(result));
        }
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        backend
            .module
            .verify()
            .expect("field receiver CFG must verify");
        let ir = llvm_fn.print_to_string().to_string();
        let entry = llvm_fn.get_first_basic_block().unwrap();
        let mut entry_ir = String::new();
        let mut instruction = entry.get_first_instruction();
        while let Some(current) = instruction {
            entry_ir.push_str(&current.print_to_string().to_string());
            instruction = current.get_next_instruction();
        }
        assert!(entry_ir.contains("field_is_ptr"), "{ir}");
        assert!(
            !entry_ir.contains("field_ptr = getelementptr")
                && !entry_ir.contains("field_flags_ptr")
                && !entry_ir.contains("load i32"),
            "receiver must be admitted first: {ir}"
        );
        let receiver_name = if is_load {
            "field_receiver"
        } else {
            "field_store_receiver"
        };
        let receiver = llvm_fn
            .get_basic_blocks()
            .into_iter()
            .find(|block| block.get_name().to_bytes() == receiver_name.as_bytes())
            .unwrap();
        let mut receiver_ir = String::new();
        let mut instruction = receiver.get_first_instruction();
        while let Some(current) = instruction {
            receiver_ir.push_str(&current.print_to_string().to_string());
            instruction = current.get_next_instruction();
        }
        assert!(receiver_ir.contains("field_flags_ptr"), "{ir}");
        assert!(receiver_ir.contains("load i32"), "{ir}");
        assert!(receiver_ir.contains("field_needs_runtime"), "{ir}");
        assert!(!receiver_ir.contains("field_ptr = getelementptr"), "{ir}");
        assert!(!receiver_ir.contains("store i64"), "{ir}");
        assert!(
            receiver
                .get_terminator()
                .unwrap()
                .print_to_string()
                .to_string()
                .contains("br i1"),
            "{ir}"
        );
        let branch = entry
            .get_terminator()
            .unwrap()
            .print_to_string()
            .to_string();
        if is_load {
            assert!(
                branch.contains("label %field_receiver, label %field_load_merge"),
                "{ir}"
            );
            assert!(
                ir.contains("field_load_value = phi i64"),
                "invalid receiver returns None: {ir}"
            );
            assert!(
                ir.contains("molt_object_field_get_ptr"),
                "runtime resolves backing and returns one owner: {ir}"
            );
            assert!(
                !ir.contains("call void @molt_inc_ref_obj"),
                "admitted immediate values need no retain: {ir}"
            );
            let payload = llvm_fn
                .get_basic_blocks()
                .into_iter()
                .find(|block| block.get_name().to_bytes() == b"field_load")
                .unwrap();
            let mut payload_ir = String::new();
            let mut instruction = payload.get_first_instruction();
            while let Some(current) = instruction {
                payload_ir.push_str(&current.print_to_string().to_string());
                instruction = current.get_next_instruction();
            }
            assert!(payload_ir.contains("field_val = load i64"), "{ir}");
            assert!(
                payload_ir.contains("field_is_ptr"),
                "missing sentinel must not escape clear-header inline loads: {ir}"
            );
            let payload_branch = payload
                .get_terminator()
                .unwrap()
                .print_to_string()
                .to_string();
            assert!(
                payload_branch.contains("br i1")
                    && payload_branch.contains("label %field_runtime, label %field_load_merge"),
                "{ir}"
            );
        } else {
            assert!(
                branch.contains("label %field_store_receiver, label %field_store_slow"),
                "{ir}"
            );
            assert!(
                ir.contains("molt_object_field_set"),
                "unproved pointer stores retain, publish and release: {ir}"
            );
            assert!(
                !ir.contains("molt_object_field_init"),
                "operation spelling cannot mint initialization proof: {ir}"
            );
            assert!(
                ir.contains("store i64"),
                "ordinary scalar assignment keeps the admitted inline fast path: {ir}"
            );
        }
    }
}

#[test]
fn lower_dynamic_get_attr_name_uses_operand_name() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "dynamic_get_attr_name".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::LoadAttr,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("get_attr_name".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_get_attr_name"), "{ir}");
    assert!(ir.contains("i64 %0, i64 %1"), "{ir}");
}

#[test]
fn lower_generic_get_attr_trusts_runtime_owned_result() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "generic_get_attr_owned".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    let mut load = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::LoadAttr,
        operands: vec![ValueId(0)],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("get_attr_generic_obj".into()),
            );
            attrs.insert("name".into(), AttrValue::Str("items".into()));
            attrs
        },
        source_span: None,
    };
    load.set_source_op_index(17);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(load);
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_get_attr_object_ic"), "{ir}");
    assert!(!ir.contains("get_attr_object_ic_inc_ref"), "{ir}");
    assert!(!ir.contains("call void @molt_inc_ref_obj"), "{ir}");
}

#[test]
fn lower_dynamic_set_attr_name_uses_operand_name() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "dynamic_set_attr_name".into(),
        vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::StoreAttr,
        operands: vec![ValueId(0), ValueId(1), ValueId(2)],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("set_attr_name".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_set_attr_name"), "{ir}");
    assert!(ir.contains("i64 %0, i64 %1, i64 %2"), "{ir}");
}

#[test]
fn lower_dynamic_del_attr_name_uses_operand_name() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "dynamic_del_attr_name".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DelAttr,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("del_attr_name".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_del_attr_name"), "{ir}");
    assert!(ir.contains("i64 %0, i64 %1"), "{ir}");
}

#[test]
fn lower_preserved_has_attr_name_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "has_attr_name_preserved".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let obj_bits = func.fresh_value();
    let name_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(obj_bits), const_none_def(name_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![obj_bits, name_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("has_attr_name".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_has_attr_name"), "{ir}");
}

#[test]
fn lower_call_method_uses_call_bind_ic_abi() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "call_method_abi".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let callable = func.fresh_value();
    let arg0 = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(callable), const_none_def(arg0)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallMethod,
        operands: vec![callable, arg0],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_call_bind_ic"), "{ir}");
    assert!(!ir.contains("molt_call_method"), "{ir}");
}

#[test]
fn lower_call_bind_preserves_callargs_builder_abi() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "call_bind_abi".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let callable = func.fresh_value();
    let builder = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(callable), const_none_def(builder)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![callable, builder],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("call_bind".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_call_bind_ic"), "{ir}");
}
