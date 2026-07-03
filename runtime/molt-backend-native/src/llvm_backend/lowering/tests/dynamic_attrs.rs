use super::*;

#[test]
fn lower_dynamic_get_attr_name_uses_operand_name() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "dynamic_get_attr_name".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
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
fn lower_dynamic_set_attr_name_uses_operand_name() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "dynamic_set_attr_name".into(),
        vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
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
    let mut func = TirFunction::new("has_attr_name_preserved".into(), vec![], TirType::DynBox);
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
    let mut func = TirFunction::new("call_method_abi".into(), vec![], TirType::DynBox);
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
    let mut func = TirFunction::new("call_bind_abi".into(), vec![], TirType::DynBox);
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
