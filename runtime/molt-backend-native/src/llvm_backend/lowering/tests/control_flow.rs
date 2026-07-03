use super::*;

#[test]
fn lower_const_and_return() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    // Build: fn f() -> i64 { return 42 }
    let mut func = TirFunction::new("const_ret".into(), vec![], TirType::I64);
    let v0 = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![v0],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(42));
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![v0] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(ir.contains("const_ret"), "function name missing from IR");
    assert!(ir.contains("42"), "constant 42 missing from IR");
    assert!(ir.contains("ret "), "return instruction missing from IR");
}

#[test]
fn lowers_exception_pop_then_dec_ref_from_shared_drop_shape() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new("exception_drop".into(), vec![], TirType::None);
    let owned = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(owned));
    let mut exception_pop = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    };
    exception_pop.attrs.insert(
        "_original_kind".into(),
        AttrValue::Str("exception_pop".into()),
    );
    entry.ops.push(exception_pop);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DecRef,
        operands: vec![owned],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    let pop_pos = ir
        .find("molt_exception_pop")
        .unwrap_or_else(|| panic!("LLVM must call molt_exception_pop; IR:\n{ir}"));
    let dec_pos = ir
        .find("molt_dec_ref_obj")
        .unwrap_or_else(|| panic!("LLVM must call molt_dec_ref_obj; IR:\n{ir}"));
    assert!(
        pop_pos < dec_pos,
        "shared ExceptionRegion drops must lower after the owning exception_pop; IR:\n{ir}"
    );
}

#[test]
fn missing_value_id_is_fatal_lowering_error() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new("missing_value".into(), vec![], TirType::I64);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::Return {
        values: vec![ValueId(99)],
    };

    let err = match try_lower_tir_to_llvm(&func, &backend) {
        Ok(_) => panic!("malformed TIR unexpectedly lowered successfully"),
        Err(err) => err,
    };
    assert_lowering_error_contains(&err, "ValueId %99 was used before being defined");
}

#[test]
fn missing_phi_argument_is_fatal_lowering_error() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new("missing_phi_arg".into(), vec![], TirType::I64);
    let join_id = func.fresh_block();
    let join_arg = func.fresh_value();

    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target: join_id,
        args: vec![],
    };
    func.blocks.insert(
        join_id,
        TirBlock {
            id: join_id,
            args: vec![TirValue {
                id: join_arg,
                ty: TirType::I64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![join_arg],
            },
        },
    );

    let err = match try_lower_tir_to_llvm(&func, &backend) {
        Ok(_) => panic!("malformed phi unexpectedly lowered successfully"),
        Err(err) => err,
    };
    assert_lowering_error_contains(&err, "phi argument index 0 is required");
}

#[test]
fn unreachable_predecessor_does_not_feed_phi() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new("dead_phi_pred".into(), vec![], TirType::DynBox);
    let join_id = func.fresh_block();
    let dead_id = func.fresh_block();
    let live_value = func.fresh_value();
    let join_arg = func.fresh_value();

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(live_value));
    entry.terminator = Terminator::Branch {
        target: join_id,
        args: vec![live_value],
    };
    func.blocks.insert(
        join_id,
        TirBlock {
            id: join_id,
            args: vec![TirValue {
                id: join_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![join_arg],
            },
        },
    );
    func.blocks.insert(
        dead_id,
        TirBlock {
            id: dead_id,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Branch {
                target: join_id,
                args: vec![ValueId(999)],
            },
        },
    );

    try_lower_tir_to_llvm(&func, &backend)
        .expect("dead TIR predecessor must not contribute to LLVM phi incoming values");
    backend
        .module
        .verify()
        .expect("dead predecessor phi lowering should verify");
}

#[test]
fn check_exception_edge_feeds_handler_phi() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);

    let mut func = TirFunction::new("check_exception_phi".into(), vec![], TirType::DynBox);
    let exit_id = func.fresh_block();
    let handler_id = func.fresh_block();
    let live_value = func.fresh_value();
    let exit_value = func.fresh_value();
    let handler_arg = func.fresh_value();

    let mut handler_attrs = AttrDict::new();
    handler_attrs.insert("value".into(), AttrValue::Int(100));

    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(live_value));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::CheckException,
        operands: vec![live_value],
        results: vec![],
        attrs: handler_attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Branch {
        target: exit_id,
        args: vec![],
    };
    func.blocks.insert(
        exit_id,
        TirBlock {
            id: exit_id,
            args: vec![],
            ops: vec![const_none_def(exit_value)],
            terminator: Terminator::Return {
                values: vec![exit_value],
            },
        },
    );
    func.blocks.insert(
        handler_id,
        TirBlock {
            id: handler_id,
            args: vec![TirValue {
                id: handler_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![handler_arg],
            },
        },
    );
    func.has_exception_handling = true;
    func.label_id_map.insert(handler_id.0, 100);

    try_lower_tir_to_llvm(&func, &backend)
        .expect("check_exception operands must feed handler block phi args");
    backend
        .module
        .verify()
        .expect("check_exception handler phi lowering should verify");
}
