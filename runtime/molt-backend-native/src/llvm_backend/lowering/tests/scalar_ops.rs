use super::*;

#[test]
fn refcount_uses_lowered_carriers_without_boxing() {
    for ty in [
        TirType::I64,
        TirType::F64,
        TirType::Bool,
        TirType::DynBox,
        TirType::Str,
    ] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let func = TirFunction::new("rc_carriers".into(), vec![], TirType::None);
        let llvm_fn =
            backend
                .module
                .add_function("rc_carriers", ctx.void_type().fn_type(&[], false), None);
        backend
            .builder
            .position_at_end(ctx.append_basic_block(llvm_fn, "entry"));
        let mut lowering = make_dummy_lowering(&backend, &func, llvm_fn);
        let value: BasicValueEnum<'_> = match ty {
            TirType::F64 => ctx.f64_type().const_float(-0.0).into(),
            TirType::Bool => ctx.bool_type().const_int(1, false).into(),
            _ => ctx.i64_type().const_int(i64::MAX as u64, false).into(),
        };
        lowering.values.insert(ValueId(0), value);
        lowering.value_types.insert(ValueId(0), ty.clone());
        for opcode in [OpCode::IncRef, OpCode::DecRef, OpCode::DelBoundary] {
            lowering.lower_op(
                func.entry_block,
                &TirOp {
                    dialect: Dialect::Molt,
                    opcode,
                    operands: vec![ValueId(0)],
                    results: vec![],
                    attrs: AttrDict::new(),
                    source_span: None,
                },
            );
        }
        backend.builder.build_return(None).unwrap();
        backend.module.verify().unwrap();
        let ir = llvm_fn.print_to_string().to_string();
        let expected = if FunctionLowering::tir_type_is_dynbox_like(&ty) {
            3
        } else {
            0
        };
        assert_eq!(ir.matches("call ").count(), expected, "{ty:?}: {ir}");
        assert!(
            !ir.contains("@molt_int_from_i64"),
            "RC must never allocate: {ir}"
        );
    }
}

#[test]
fn box_and_reference_unbox_each_mint_their_own_heap_owner() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let func = TirFunction::new("owned_box".into(), vec![TirType::DynBox], TirType::Str);
    let llvm_fn = backend.module.add_function(
        "owned_box",
        ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
        None,
    );
    backend
        .builder
        .position_at_end(ctx.append_basic_block(llvm_fn, "entry"));
    let mut lowering = make_dummy_lowering(&backend, &func, llvm_fn);
    lowering
        .values
        .insert(ValueId(0), llvm_fn.get_first_param().unwrap());
    lowering.value_types.insert(ValueId(0), TirType::DynBox);
    let mut op = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BoxVal,
        operands: vec![ValueId(0)],
        results: vec![ValueId(1)],
        attrs: AttrDict::new(),
        source_span: None,
    };
    lowering.emit_box(&op);
    op.opcode = OpCode::UnboxVal;
    op.operands = vec![ValueId(1)];
    op.results = vec![ValueId(2)];
    lowering.value_types.insert(ValueId(2), TirType::Str);
    lowering.emit_unbox(&op);
    backend
        .builder
        .build_return(Some(&lowering.resolve(ValueId(2))))
        .unwrap();
    backend.module.verify().unwrap();
    let ir = llvm_fn.print_to_string().to_string();
    assert_eq!(ir.matches("call void @molt_inc_ref_obj").count(), 2, "{ir}");
}

#[test]
fn discarded_representation_results_preserve_only_boxing_allocation_effects() {
    for (opcode, ty) in [
        (OpCode::BoxVal, TirType::I64),
        (OpCode::BoxVal, TirType::DynBox),
        (OpCode::UnboxVal, TirType::DynBox),
    ] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let func = TirFunction::new(
            "discarded_representation".into(),
            vec![ty.clone()],
            TirType::None,
        );
        let llvm_fn = backend.module.add_function(
            "discarded_representation",
            ctx.void_type().fn_type(&[ctx.i64_type().into()], false),
            None,
        );
        backend
            .builder
            .position_at_end(ctx.append_basic_block(llvm_fn, "entry"));
        let mut lowering = make_dummy_lowering(&backend, &func, llvm_fn);
        lowering
            .values
            .insert(ValueId(0), llvm_fn.get_first_param().unwrap());
        lowering.value_types.insert(ValueId(0), ty.clone());
        lowering.lower_op(
            func.entry_block,
            &TirOp {
                dialect: Dialect::Molt,
                opcode,
                operands: vec![ValueId(0)],
                results: vec![],
                attrs: AttrDict::new(),
                source_span: None,
            },
        );
        backend.builder.build_return(None).unwrap();
        backend.module.verify().unwrap();
        let ir = llvm_fn.print_to_string().to_string();
        let allocates = opcode == OpCode::BoxVal && ty == TirType::I64;
        assert_eq!(
            ir.matches("call i64 @molt_int_from_i64").count(),
            usize::from(allocates),
            "{ir}"
        );
        assert_eq!(
            ir.matches("call void @molt_dec_ref_obj").count(),
            usize::from(allocates),
            "{ir}"
        );
        assert!(!ir.contains("call void @molt_inc_ref_obj"), "{ir}");
        assert!(!ir.contains("call i64 @molt_int_as_i64"), "{ir}");
    }
}

#[test]
fn integer_unbox_and_trampoline_share_full_width_fixed_block_decoder() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let func = TirFunction::new("decode_i64".into(), vec![TirType::DynBox], TirType::I64);
    let llvm_fn = backend.module.add_function(
        "decode_i64",
        ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
        None,
    );
    let entry = ctx.append_basic_block(llvm_fn, "entry");
    backend.builder.position_at_end(entry);
    let mut lowering = make_dummy_lowering(&backend, &func, llvm_fn);
    let raw = llvm_fn.get_first_param().unwrap().into_int_value();
    lowering.values.insert(ValueId(0), raw.into());
    lowering.value_types.insert(ValueId(0), TirType::DynBox);
    lowering.value_types.insert(ValueId(1), TirType::I64);
    lowering.emit_unbox(&TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::UnboxVal,
        operands: vec![ValueId(0)],
        results: vec![ValueId(1)],
        attrs: AttrDict::new(),
        source_span: None,
    });
    let trampoline = unbox_dynbox_to_param_ty_with_builder(
        &backend.builder,
        &ctx,
        &backend.module,
        raw,
        &TirType::I64,
    );
    backend.builder.build_return(Some(&trampoline)).unwrap();
    backend.module.verify().unwrap();
    let ir = llvm_fn.print_to_string().to_string();
    assert_eq!(ir.matches("call i64 @molt_int_as_i64").count(), 2, "{ir}");
    assert_eq!(
        llvm_fn.count_basic_blocks(),
        1,
        "phi insertions must remain in their predecessor"
    );
    assert!(
        !ir.contains("sign_extend") && !ir.contains("payload"),
        "{ir}"
    );
}

#[test]
fn heap_literals_materialize_an_owned_result_after_an_equal_result_is_dropped() {
    for (opcode, result_ty, runtime_name, payload) in [
        (
            OpCode::ConstStr,
            TirType::Str,
            "molt_string_from_bytes",
            AttrValue::Bytes(b"same\0literal".to_vec()),
        ),
        (
            OpCode::ConstBytes,
            TirType::Bytes,
            "molt_bytes_from_bytes",
            AttrValue::Bytes(vec![0xff, 0, 0x80]),
        ),
        (
            OpCode::ConstBigInt,
            TirType::DynBox,
            "molt_bigint_from_str",
            AttrValue::Str("123456789012345678901234567890".into()),
        ),
    ] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let func = TirFunction::new("literal_ownership".into(), vec![], result_ty.clone());
        let llvm_fn = backend.module.add_function(
            "literal_ownership",
            ctx.i64_type().fn_type(&[], false),
            None,
        );
        let entry = ctx.append_basic_block(llvm_fn, "entry");
        backend.builder.position_at_end(entry);
        let mut lowering = make_dummy_lowering(&backend, &func, llvm_fn);
        let mut attrs = AttrDict::new();
        let payload_key = if opcode == OpCode::ConstBigInt {
            "s_value"
        } else {
            "bytes"
        };
        attrs.insert(payload_key.into(), payload);
        for result in [ValueId(0), ValueId(1)] {
            lowering.lower_op(
                func.entry_block,
                &TirOp {
                    dialect: Dialect::Molt,
                    opcode,
                    operands: vec![],
                    results: vec![result],
                    attrs: attrs.clone(),
                    source_span: None,
                },
            );
            assert_eq!(lowering.value_types[&result], result_ty);
            if result == ValueId(0) {
                lowering.lower_op(
                    func.entry_block,
                    &TirOp {
                        dialect: Dialect::Molt,
                        opcode: OpCode::DecRef,
                        operands: vec![result],
                        results: vec![],
                        attrs: AttrDict::new(),
                        source_span: None,
                    },
                );
            }
        }
        assert_ne!(lowering.values[&ValueId(0)], lowering.values[&ValueId(1)]);
        let result = lowering.ensure_i64(lowering.values[&ValueId(1)]);
        backend.builder.build_return(Some(&result)).unwrap();
        assert!(lowering.diagnostics.borrow().is_empty());
        backend.module.verify().expect("literal module verifies");
        let ir = llvm_fn.print_to_string().to_string();
        let calls: Vec<_> = ir.lines().filter(|line| line.contains("call ")).collect();
        assert_eq!(
            calls.len(),
            3,
            "one owned call per result and one drop: {ir}"
        );
        assert!(calls[0].contains(&format!("@{runtime_name}(")), "{ir}");
        assert!(calls[1].contains("@molt_dec_ref_obj("), "{ir}");
        assert!(calls[2].contains(&format!("@{runtime_name}(")), "{ir}");
        let materializer = backend.module.get_function(runtime_name).unwrap();
        let ptr_ty = ctx.ptr_type(inkwell::AddressSpace::default());
        let expected_abi = if opcode == OpCode::ConstBigInt {
            ctx.i64_type()
                .fn_type(&[ptr_ty.into(), ctx.i64_type().into()], false)
        } else {
            ctx.i32_type().fn_type(
                &[ptr_ty.into(), ctx.i64_type().into(), ptr_ty.into()],
                false,
            )
        };
        assert_eq!(materializer.get_type(), expected_abi, "{runtime_name}");
    }
}

#[test]
fn repeated_attribute_name_literals_use_owned_string_materialization() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let func = TirFunction::new("literal_names".into(), vec![], TirType::Str);
    let llvm_fn =
        backend
            .module
            .add_function("literal_names", ctx.i64_type().fn_type(&[], false), None);
    let entry = ctx.append_basic_block(llvm_fn, "entry");
    backend.builder.position_at_end(entry);
    let mut lowering = make_dummy_lowering(&backend, &func, llvm_fn);
    let mut results = Vec::new();
    for _ in 0..2 {
        results.push(lowering.with_owned_name("same_name", |this, name_bits| {
            let import = this.ensure_runtime_i64_fn("molt_module_import", 1);
            this.backend
                .builder
                .build_call(import, &[name_bits.into()], "name_consumer")
                .unwrap()
                .try_as_basic_value()
                .unwrap_basic()
        }));
    }
    let [first, second] = results.as_slice() else {
        unreachable!()
    };
    assert_ne!(first, second);
    let result = lowering.ensure_i64(*second);
    backend.builder.build_return(Some(&result)).unwrap();
    backend
        .module
        .verify()
        .expect("name literal module verifies");
    let ir = llvm_fn.print_to_string().to_string();
    assert_eq!(ir.matches("@molt_string_from_bytes(").count(), 2, "{ir}");
    assert_eq!(ir.matches("@molt_dec_ref_obj(").count(), 2, "{ir}");
    assert_eq!(ir.matches("icmp eq i32").count(), 2, "{ir}");
    assert!(!ir.contains("@molt_bytes_from_bytes("), "{ir}");
}

#[test]
fn synthesized_name_consumers_gate_calls_and_release_only_admitted_names() {
    fn block_text(block: BasicBlock<'_>) -> String {
        let mut text = String::new();
        let mut instruction = block.get_first_instruction();
        while let Some(current) = instruction {
            text.push_str(&current.print_to_string().to_string());
            text.push('\n');
            instruction = current.get_next_instruction();
        }
        text
    }
    for (opcode, key, runtime, operands, has_result) in [
        (OpCode::LoadAttr, "name", "molt_get_attr_name", 1, true),
        (OpCode::StoreAttr, "name", "molt_set_attr_name", 2, false),
        (OpCode::DelAttr, "name", "molt_del_attr_name", 1, false),
        (OpCode::Import, "module", "molt_module_import", 0, true),
        (OpCode::Import, "s_value", "molt_module_import", 0, true),
        (OpCode::Import, "_var", "molt_module_import", 0, true),
        (OpCode::CallBuiltin, "name", "molt_call_builtin", 1, true),
    ] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let func = TirFunction::new("checked_name".into(), vec![], TirType::DynBox);
        let llvm_fn =
            backend
                .module
                .add_function("checked_name", ctx.i64_type().fn_type(&[], false), None);
        let entry = ctx.append_basic_block(llvm_fn, "entry");
        backend.builder.position_at_end(entry);
        let mut lowering = make_dummy_lowering(&backend, &func, llvm_fn);
        for operand in [ValueId(0), ValueId(1)] {
            lowering.lower_op(func.entry_block, &const_none_def(operand));
        }
        let result_id = ValueId(2);
        lowering.lower_op(
            func.entry_block,
            &TirOp {
                dialect: Dialect::Molt,
                opcode,
                operands: (0..operands).map(ValueId).collect(),
                results: if has_result { vec![result_id] } else { vec![] },
                attrs: AttrDict::from([(key.into(), AttrValue::Str("same_name".into()))]),
                source_span: None,
            },
        );
        // The enclosing operation, not a private return, owns exception routing.
        lowering.lower_op(
            func.entry_block,
            &TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::CheckException,
                operands: vec![],
                results: vec![],
                attrs: AttrDict::new(),
                source_span: None,
            },
        );
        let result = lowering.values[&if has_result { result_id } else { ValueId(0) }];
        backend.builder.build_return(Some(&result)).unwrap();
        backend
            .module
            .verify()
            .expect("checked name CFG must verify");
        let ir = llvm_fn.print_to_string().to_string();
        let source = block_text(entry);
        let success = block_text(
            llvm_fn
                .get_basic_blocks()
                .into_iter()
                .find(|block| block.get_name().to_bytes() == b"owned_name_success0")
                .unwrap(),
        );
        let merge = block_text(
            llvm_fn
                .get_basic_blocks()
                .into_iter()
                .find(|block| block.get_name().to_bytes() == b"owned_name_merge0")
                .unwrap(),
        );
        assert!(source.contains("icmp eq i32 %sfb, 0"), "{ir}");
        assert!(
            source.contains(
                "br i1 %owned_name_ok, label %owned_name_success0, label %owned_name_merge0"
            ),
            "{ir}"
        );
        assert!(!source.contains(&format!("@{runtime}(")), "{ir}");
        assert!(!source.contains("@molt_dec_ref_obj("), "{ir}");
        assert_eq!(success.matches(&format!("@{runtime}(")).count(), 1, "{ir}");
        assert_eq!(
            ir.matches("@molt_dec_ref_obj(i64 %str_bits)").count(),
            1,
            "{ir}"
        );
        assert!(
            success.find(&format!("@{runtime}(")).unwrap()
                < success.find("@molt_dec_ref_obj(").unwrap(),
            "{ir}"
        );
        assert!(!success.contains("ret "), "{ir}");
        assert!(
            merge.contains(&format!("[ {}, %entry ]", nanbox::QNAN | nanbox::TAG_NONE)),
            "{ir}"
        );
        assert!(merge.contains("@molt_exception_pending()"), "{ir}");
        assert!(
            !merge.contains("@molt_dec_ref_obj(") && !merge.contains("ptr_unbox"),
            "{ir}"
        );
        if opcode == OpCode::CallBuiltin {
            assert!(!source.contains("@molt_callargs_new("), "{ir}");
            assert!(success.contains("@molt_callargs_new("), "{ir}");
        }
    }
}

/// Exercise actual opcode dispatch with an explicit physical operand carrier.
/// The leaf returns its emitted bits solely for IR inspection. Supplied result
/// facts test lane selection; they do not prove the range analysis itself.
fn lower_unary_carrier_fixture(
    opcode: OpCode,
    operand_ty: TirType,
    operand_bits: u64,
    result_repr: Option<crate::Repr>,
) -> (String, TirType) {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let func = TirFunction::new("unary_carrier".into(), vec![], TirType::DynBox);
    let llvm_fn =
        backend
            .module
            .add_function("unary_carrier", ctx.i64_type().fn_type(&[], false), None);
    let entry = ctx.append_basic_block(llvm_fn, "entry");
    backend.builder.position_at_end(entry);
    let mut lowering = make_dummy_lowering(&backend, &func, llvm_fn);
    let operand = ValueId(0);
    let result = ValueId(1);
    let value: BasicValueEnum<'_> = match operand_ty {
        TirType::Bool => ctx.bool_type().const_int(operand_bits, false).into(),
        TirType::F64 => ctx
            .f64_type()
            .const_float(f64::from_bits(operand_bits))
            .into(),
        _ => ctx.i64_type().const_int(operand_bits, false).into(),
    };
    lowering.values.insert(operand, value);
    lowering.value_types.insert(operand, operand_ty);
    if let Some(repr) = result_repr {
        lowering.repr_facts.repr_by_value.insert(result, repr);
    }
    lowering.lower_op(
        func.entry_block,
        &TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![operand],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        },
    );
    let result_ty = lowering.value_types[&result].clone();
    let bits = lowering.ensure_i64(lowering.values[&result]);
    backend.builder.build_return(Some(&bits)).unwrap();
    assert!(lowering.diagnostics.borrow().is_empty());
    backend
        .module
        .verify()
        .expect("unary carrier module verifies");
    (llvm_fn.print_to_string().to_string(), result_ty)
}

#[test]
fn unary_runtime_fallback_boxes_bool_arguments() {
    for (opcode, symbol) in [
        (OpCode::Neg, "molt_neg"),
        (OpCode::Pos, "molt_pos"),
        (OpCode::BitNot, "molt_invert"),
    ] {
        for value in [false, true] {
            let (ir, result_ty) =
                lower_unary_carrier_fixture(opcode, TirType::Bool, u64::from(value), None);
            let boxed = nanbox::QNAN | nanbox::TAG_BOOL | u64::from(value);
            assert!(
                ir.contains(&format!("@{symbol}(i64 {boxed})")),
                "{opcode:?} must pass the actual Bool tag, not raw 0/1: {ir}"
            );
            assert_eq!(result_ty, TirType::DynBox, "{opcode:?}: {ir}");
        }
    }
}

#[test]
fn unary_runtime_fallback_preserves_boxed_and_none_arguments() {
    for (opcode, symbol) in [
        (OpCode::Neg, "molt_neg"),
        (OpCode::Pos, "molt_pos"),
        (OpCode::BitNot, "molt_invert"),
        (OpCode::Not, "molt_not"),
    ] {
        for (operand_ty, boxed) in [
            (TirType::DynBox, nanbox::QNAN | nanbox::TAG_INT | 7),
            (TirType::None, nanbox::QNAN | nanbox::TAG_NONE),
        ] {
            let (ir, result_ty) = lower_unary_carrier_fixture(opcode, operand_ty, boxed, None);
            assert!(
                ir.contains(&format!("@{symbol}(i64 {boxed})")),
                "{opcode:?} must dispatch the boxed object unchanged: {ir}"
            );
            assert_eq!(result_ty, TirType::DynBox, "{opcode:?}: {ir}");
        }
    }
}

#[test]
fn unary_runtime_fallback_materializes_float_arguments() {
    for (opcode, symbol) in [(OpCode::BitNot, "molt_invert"), (OpCode::Not, "molt_not")] {
        let bits = 1.25_f64.to_bits();
        let (ir, result_ty) = lower_unary_carrier_fixture(opcode, TirType::F64, bits, None);
        assert!(ir.contains(&format!("@{symbol}(i64 {bits})")), "{ir}");
        assert_eq!(result_ty, TirType::DynBox, "{ir}");
    }
}

#[test]
fn unary_neg_without_result_range_proof_boxes_full_i64_before_runtime() {
    for value in [i64::MIN, i64::MAX, -(1_i64 << 46), 42] {
        let (ir, result_ty) =
            lower_unary_carrier_fixture(OpCode::Neg, TirType::I64, value as u64, None);
        assert!(
            ir.contains("@molt_neg(i64 "),
            "unproved neg must use runtime: {ir}"
        );
        assert!(
            ir.contains(&format!("@molt_int_from_i64(i64 {value})")),
            "fallback must preserve the full i64 before runtime dispatch: {ir}"
        );
        assert!(
            !ir.contains(&format!("@molt_neg(i64 {value})")),
            "raw integer bits are not boxed runtime operands: {ir}"
        );
        assert_eq!(result_ty, TirType::DynBox, "{ir}");
    }
}

#[test]
fn unary_proven_numeric_lanes_keep_raw_results() {
    for (opcode, operand, expected, result_repr) in [
        (OpCode::Neg, 42_i64, -42_i64, Some(crate::Repr::RawI64Safe)),
        (OpCode::Pos, i64::MIN, i64::MIN, None),
        (OpCode::BitNot, i64::MIN, i64::MAX, None),
    ] {
        let (ir, result_ty) =
            lower_unary_carrier_fixture(opcode, TirType::I64, operand as u64, result_repr);
        assert!(ir.contains(&format!("ret i64 {expected}")), "{ir}");
        assert!(
            !ir.contains("call "),
            "proven raw unary must not dispatch: {ir}"
        );
        assert_eq!(result_ty, TirType::I64);
    }
    for opcode in [OpCode::Neg, OpCode::Pos] {
        for value in [1.25_f64, -0.0_f64] {
            let expected = if opcode == OpCode::Neg { -value } else { value };
            let (ir, result_ty) =
                lower_unary_carrier_fixture(opcode, TirType::F64, value.to_bits(), None);
            assert!(
                ir.contains(&format!("ret i64 {}", expected.to_bits() as i64)),
                "float unary must preserve the sign of zero: {ir}"
            );
            assert!(!ir.contains("call "), "{ir}");
            assert_eq!(result_ty, TirType::F64);
        }
    }
    for value in [false, true] {
        let (ir, result_ty) =
            lower_unary_carrier_fixture(OpCode::Not, TirType::Bool, u64::from(value), None);
        assert!(
            ir.contains(&format!("ret i64 {}", u64::from(!value))),
            "{ir}"
        );
        assert!(!ir.contains("call "), "{ir}");
        assert_eq!(result_ty, TirType::Bool);
    }
}

#[test]
fn lower_i64_comparison() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);

    // Build: fn lt(a: i64, b: i64) -> bool { return a < b }
    let mut func = TirFunction::new(
        "cmp_lt".into(),
        vec![TirType::I64, TirType::I64],
        TirType::Bool,
    );
    let v_result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Lt,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![v_result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![v_result],
    };

    // The raw `icmp slt` path needs both operands proven exact-i64 carriers;
    // an unproven `i64` parameter carries boxed (`DynBox`) and dispatches the
    // comparison through the runtime. Prove the two parameters here.
    let mut facts = crate::representation_plan::LlvmReprFacts::default();
    for v in [ValueId(0), ValueId(1)] {
        facts.repr_by_value.insert(v, crate::Repr::RawI64Safe);
    }
    backend.function_repr_facts.insert(func.name.clone(), facts);

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(
        ir.contains("icmp slt"),
        "expected signed less-than comparison in IR: {}",
        ir
    );
}

#[test]
fn lower_box_i64() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);

    // Build: fn box_it(x: i64) -> DynBox { return box(x) }
    let mut func = TirFunction::new("box_i64".into(), vec![TirType::I64], TirType::DynBox);
    let v_boxed = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BoxVal,
        operands: vec![ValueId(0)],
        results: vec![v_boxed],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![v_boxed],
    };

    // `box(x)` emits the NaN-boxing arithmetic only when `x` is a RAW i64.
    // An unproven `i64` parameter carries already-boxed (`DynBox`), for which
    // `box` is a no-op; prove the parameter raw so the box path is exercised.
    let mut facts = crate::representation_plan::LlvmReprFacts::default();
    facts
        .repr_by_value
        .insert(ValueId(0), crate::Repr::RawI64Safe);
    backend.function_repr_facts.insert(func.name.clone(), facts);

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    // Should contain the NaN-boxing OR operations
    assert!(
        ir.contains("or i64"),
        "expected NaN-boxing OR in IR: {}",
        ir
    );
    assert!(
        ir.contains("and i64"),
        "expected NaN-boxing AND mask in IR: {}",
        ir
    );
}

#[test]
fn masked_shift_loop_phi_promoted_to_raw_i64_lane() {
    // #43 end-to-end (the perf payoff the value-range phi narrowing exists
    // for): a `DynBox`-declared loop-header phi that the representation plan
    // proves `RawI64Safe` must be carried as a raw `I64` so the in-loop
    // `<<`/`&` emit raw machine `shl`/`and` instead of the boxed
    // `molt_lshift`/`molt_bit_and` runtime. `type_refine` leaves the masked
    // accumulator `DynBox` (its inline-window fit is a value-range-only fact),
    // so without `effective_block_arg_type`'s DynBox->I64 promotion the phi
    // carries boxed and every iteration round-trips through the runtime — the
    // exact regression this guards.
    //
    // Shape:  s_phi: DynBox = phi[ 1 (preheader), band (back-edge) ]
    //         shl  = s_phi << 1
    //         band = shl & MASK            (MASK = 2**32 - 1)
    //         -> header(band)
    // with the plan proving s_phi / shl / band all RawI64Safe.
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);

    let mut func = TirFunction::new("masked_shift".into(), vec![], TirType::None);
    let s_start = func.fresh_value(); // ConstInt 1
    let mask_c = func.fresh_value(); // ConstInt (2**32 - 1)
    let one_c = func.fresh_value(); // ConstInt 1 (shift count)
    let s_phi = func.fresh_value(); // header phi (DynBox-declared)
    let shl = func.fresh_value(); // s_phi << 1
    let band = func.fresh_value(); // shl & MASK

    let header = func.fresh_block();
    let body = func.fresh_block();
    let exit = func.fresh_block();

    let mk_int = |result: ValueId, v: i64| TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![result],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(v));
            m
        },
        source_span: None,
    };
    let mk_bin = |opcode: OpCode, a: ValueId, b: ValueId, r: ValueId| TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![a, b],
        results: vec![r],
        attrs: AttrDict::new(),
        source_span: None,
    };
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops = vec![
            mk_int(s_start, 1),
            mk_int(mask_c, (1i64 << 32) - 1),
            mk_int(one_c, 1),
        ];
        entry.terminator = Terminator::Branch {
            target: header,
            args: vec![s_start],
        };
    }
    // The phi is DECLARED DynBox (as type_refine leaves the masked accumulator).
    func.blocks.insert(
        header,
        TirBlock {
            id: header,
            args: vec![TirValue {
                id: s_phi,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Branch {
                target: body,
                args: vec![],
            },
        },
    );
    func.loop_roles
        .insert(header, crate::tir::blocks::LoopRole::LoopHeader);
    func.blocks.insert(
        body,
        TirBlock {
            id: body,
            args: vec![],
            ops: vec![
                mk_bin(OpCode::Shl, s_phi, one_c, shl),
                mk_bin(OpCode::BitAnd, shl, mask_c, band),
            ],
            terminator: Terminator::Branch {
                target: header,
                args: vec![band],
            },
        },
    );
    func.blocks.insert(
        exit,
        TirBlock {
            id: exit,
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        },
    );
    func.loop_roles
        .insert(exit, crate::tir::blocks::LoopRole::LoopEnd);

    // The plan proves the masked accumulator chain RawI64Safe (what the
    // value-range phi narrowing yields end to end). The ConstInts are I64 by
    // their own lowering; the proof here is for the phi + the two op results.
    let mut facts = crate::representation_plan::LlvmReprFacts::default();
    for v in [s_phi, shl, band] {
        facts.repr_by_value.insert(v, crate::Repr::RawI64Safe);
    }
    backend.function_repr_facts.insert(func.name.clone(), facts);

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(
        ir.contains("shl i64"),
        "masked accumulator shift must lower to a RAW machine `shl i64`, not \
             the boxed runtime. IR:\n{ir}"
    );
    assert!(
        !ir.contains("@molt_lshift"),
        "a RawI64Safe-proven masked shift must NOT call the boxed `molt_lshift`. \
             IR:\n{ir}"
    );
    assert!(
        !ir.contains("@molt_bit_and"),
        "a RawI64Safe-proven masked `& MASK` must NOT call the boxed \
             `molt_bit_and`. IR:\n{ir}"
    );
    // The header phi must be a raw `i64` phi (promoted from its DynBox
    // declaration) so the back-edge carries the raw masked value.
    assert!(
        ir.contains("phi i64"),
        "the RawI64Safe masked accumulator phi must be a raw `i64` phi. IR:\n{ir}"
    );
}

// ── RPO algorithm tests ──
//
// The RPO algorithm is exercised by the shared IR tests in
// `runtime/molt-ir/src/tir/dominators/tests/rpo_tests.rs`, which call into
// [`crate::tir::dominators::executable_reverse_postorder`] directly with synthetic CFGs covering
// diamonds, loops, switches, deep chains, self-loops, and unreachable
// blocks. They run without an LLVM feature or backend build.

#[test]
fn lower_codegen_partition_emits_real_llvm_noinline_attribute() {
    for partitioned in [false, true] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut function = TirFunction::new("__molt_chunk_v1_user".into(), vec![], TirType::None);
        function.attrs.insert(
            crate::tir::function::CODEGEN_PARTITION_ATTR.into(),
            AttrValue::Bool(partitioned),
        );
        function
            .blocks
            .get_mut(&function.entry_block)
            .unwrap()
            .terminator = Terminator::Return { values: vec![] };
        let lowered = lower_tir_to_llvm(&function, &backend);
        let kind = Attribute::get_named_enum_kind_id("noinline");
        assert_ne!(kind, 0, "LLVM must expose its noinline attribute");
        assert_eq!(
            lowered
                .get_enum_attribute(AttributeLoc::Function, kind)
                .is_some(),
            partitioned
        );
    }
}
