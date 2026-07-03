use super::*;

#[test]
fn lower_call_guarded_uses_runtime_callable_dispatch_even_with_known_target() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    let _target = backend.module.add_function(
        "guarded_target",
        ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
        Some(inkwell::module::Linkage::External),
    );
    backend
        .function_param_types
        .insert("guarded_target".to_string(), vec![TirType::DynBox]);
    backend
        .function_return_types
        .insert("guarded_target".to_string(), TirType::DynBox);

    let mut func = TirFunction::new("guarded_call_abi".into(), vec![], TirType::DynBox);
    let callable = func.fresh_value();
    let arg0 = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(callable), const_none_def(arg0)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Call,
        operands: vec![callable, arg0],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("call_guarded".into()),
            );
            attrs.insert("s_value".into(), AttrValue::Str("guarded_target".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(ir.contains("molt_call_func_fast1"), "{ir}");
    assert!(!ir.contains("call i64 @guarded_target"), "{ir}");
}

#[test]
fn lower_import_uses_var_attr_fallback_for_module_name() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("import_var_fallback".into(), vec![], TirType::DynBox);
    let imported = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let mut attrs = AttrDict::new();
    attrs.insert("_var".into(), AttrValue::Str("pathlib".into()));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Import,
        operands: vec![],
        results: vec![imported],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![imported],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_module_import"), "{ir}");
}

#[test]
fn lower_direct_container_builders_box_raw_i64_elements() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("container_builder_boxing".into(), vec![], TirType::DynBox);
    let raw = func.fresh_value();
    let key = func.fresh_value();
    let list = func.fresh_value();
    let tuple = func.fresh_value();
    let set = func.fresh_value();
    let dict = func.fresh_value();
    let ret = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_int_def(raw, 2));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![key],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str("k".into()));
            attrs
        },
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildList,
        operands: vec![raw],
        results: vec![list],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildTuple,
        operands: vec![raw],
        results: vec![tuple],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildSet,
        operands: vec![raw],
        results: vec![set],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildDict,
        operands: vec![key, raw],
        results: vec![dict],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(const_none_def(ret));
    entry.terminator = Terminator::Return { values: vec![ret] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    let boxed_two = "9221401712017801218";
    assert!(
        ir.matches(boxed_two).count() >= 4,
        "each direct container builder must append boxed int bits; IR:\n{ir}"
    );
    assert!(
        !ir.contains("molt_list_builder_append(i64 %list, i64 2)"),
        "{ir}"
    );
    assert!(
        !ir.contains("molt_set_builder_append(i64 %set_builder, i64 2)"),
        "{ir}"
    );
    assert!(
        !ir.contains("molt_dict_builder_append(i64 %dict_builder, i64 %str_bits, i64 2)"),
        "{ir}"
    );
}

#[test]
fn lower_preserved_container_builders_use_void_append_abi() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "preserved_container_builder_append_abi".into(),
        vec![],
        TirType::DynBox,
    );
    let raw = func.fresh_value();
    let key = func.fresh_value();
    let list = func.fresh_value();
    let tuple = func.fresh_value();
    let set = func.fresh_value();
    let dict = func.fresh_value();
    let ret = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_int_def(raw, 2));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![key],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str("k".into()));
            attrs
        },
        source_span: None,
    });
    for (kind, operands, result) in [
        ("list_new", vec![raw], list),
        ("tuple_new", vec![raw], tuple),
        ("set_new", vec![raw], set),
        ("dict_new", vec![key, raw], dict),
    ] {
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands,
            results: vec![result],
            attrs: {
                let mut attrs = AttrDict::new();
                attrs.insert("_original_kind".into(), AttrValue::Str(kind.into()));
                attrs
            },
            source_span: None,
        });
    }
    entry.ops.push(const_none_def(ret));
    entry.terminator = Terminator::Return { values: vec![ret] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    backend.module.verify().expect("module should verify");
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("call void @molt_list_builder_append"), "{ir}");
    assert!(ir.contains("call void @molt_dict_builder_append"), "{ir}");
    assert!(ir.contains("call void @molt_set_builder_append"), "{ir}");
}

#[test]
#[should_panic(expected = "call_method_ic supports at most 4 positional args")]
fn lower_call_method_ic_rejects_over_ic4_arity() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "call_method_ic_too_many_args".into(),
        vec![],
        TirType::DynBox,
    );
    let mut operands = Vec::new();
    for _ in 0..6 {
        let value = func.fresh_value();
        func.blocks
            .get_mut(&func.entry_block)
            .unwrap()
            .ops
            .push(const_none_def(value));
        operands.push(value);
    }
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallMethodIc,
        operands,
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("method".into(), AttrValue::Str("m".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let _ = lower_tir_to_llvm(&func, &backend);
}

#[test]
fn lower_call_method_ic_preserves_central_no_willreturn_declaration() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("call_method_ic_attr_reuse".into(), vec![], TirType::DynBox);
    let recv = func.fresh_value();
    let arg = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(recv));
    entry.ops.push(const_none_def(arg));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallMethodIc,
        operands: vec![recv, arg],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("method".into(), AttrValue::Str("m".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_call_method_ic1"), "{ir}");
    let runtime_fn = backend
        .module
        .get_function("molt_call_method_ic1")
        .expect("central method IC runtime import should exist");
    assert!(has_fn_attr(runtime_fn, "nounwind"));
    assert!(
        lacks_fn_attr(runtime_fn, "willreturn"),
        "method IC dispatch executes arbitrary user code"
    );
}

#[test]
#[should_panic(expected = "call_super_method_ic supports at most 4 positional args")]
fn lower_call_super_method_ic_rejects_over_ic4_arity() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "call_super_method_ic_too_many_args".into(),
        vec![],
        TirType::DynBox,
    );
    let mut operands = Vec::new();
    for _ in 0..7 {
        let value = func.fresh_value();
        func.blocks
            .get_mut(&func.entry_block)
            .unwrap()
            .ops
            .push(const_none_def(value));
        operands.push(value);
    }
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CallSuperMethodIc,
        operands,
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("method".into(), AttrValue::Str("m".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let _ = lower_tir_to_llvm(&func, &backend);
}

#[test]
fn lower_class_def_boxes_raw_i64_attribute_values() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("class_def_boxed_attrs".into(), vec![], TirType::DynBox);
    let name = func.fresh_value();
    let base = func.fresh_value();
    let attr_key = func.fresh_value();
    let attr_value = func.fresh_value();
    let class_obj = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![name],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str("C".into()));
            attrs
        },
        source_span: None,
    });
    entry.ops.push(const_none_def(base));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstStr,
        operands: vec![],
        results: vec![attr_key],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str("y".into()));
            attrs
        },
        source_span: None,
    });
    entry.ops.push(const_int_def(attr_value, 2));
    let mut attrs = AttrDict::new();
    attrs.insert("_original_kind".into(), AttrValue::Str("class_def".into()));
    attrs.insert("s_value".into(), AttrValue::Str("1,1,0,0,0".into()));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![name, base, attr_key, attr_value],
        results: vec![class_obj],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![class_obj],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_guarded_class_def"), "{ir}");
    assert!(
        ir.contains("9221401712017801218"),
        "class_def attr values must be boxed before array storage; IR:\n{ir}"
    );
    assert!(!ir.contains("store i64 2, ptr %class_attr_ptr_1"), "{ir}");
}

#[test]
fn lower_preserved_dict_update_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("dict_update_preserved".into(), vec![], TirType::DynBox);
    let dict_bits = func.fresh_value();
    let other_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(dict_bits), const_none_def(other_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![dict_bits, other_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("dict_update".into()),
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
    assert!(ir.contains("molt_dict_update"), "{ir}");
}
