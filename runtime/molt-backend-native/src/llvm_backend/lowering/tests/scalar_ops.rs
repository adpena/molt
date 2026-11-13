use super::*;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionBuilderExt;
#[cfg(feature = "llvm")]
use crate::llvm_backend::execution_abi::ExecutionModuleExt;
use molt_ir::tir::op_kinds_generated::ExceptionEdgeKind;

#[test]
fn none_annotation_cannot_replace_a_borrowed_boxed_word() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "none_annotation_word".into(),
        vec![TirType::None],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let parameter = func.blocks[&func.entry_block].args[0].id;
    let boxed = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::BoxVal,
        operands: vec![parameter],
        results: vec![boxed],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![boxed],
    };
    backend.function_repr_facts.insert(
        func.name.clone(),
        crate::representation_plan::ProducerCarrierPlan::build(&func),
    );
    let function = lower_tir_to_llvm(&func, &backend);
    backend.module.verify().unwrap();
    let ir = function.print_to_string().to_string();
    assert!(
        ir.contains_execution("@__molt_inline_retain(i64 %0,"),
        "annotation must retain the actual word: {ir}"
    );
    assert!(
        ir.contains_execution("ret i64 %0"),
        "annotation must preserve the actual word: {ir}"
    );
}

#[test]
fn annotation_only_unbox_preserves_a_boxed_owner_and_its_refcounts() {
    for (annotation, wire_type) in [
        (TirType::I64, "i64"),
        (TirType::F64, "f64"),
        (TirType::Bool, "bool"),
    ] {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "unproven_unbox".into(),
            vec![annotation.clone()],
            annotation.clone(),
            molt_ir::FunctionReturnAbi::Value,
        );
        let result = func.fresh_value();
        func.value_types.insert(result, annotation);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            origin: None,
            runtime: None,
            dialect: Dialect::Molt,
            opcode: OpCode::UnboxVal,
            operands: vec![ValueId(0)],
            results: vec![result],
            attrs: [("type".into(), AttrValue::Str(wire_type.into()))]
                .into_iter()
                .collect(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            origin: None,
            runtime: None,
            dialect: Dialect::Molt,
            opcode: OpCode::DecRef,
            operands: vec![result],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![ValueId(0)],
        };
        backend.function_repr_facts.insert(
            func.name.clone(),
            crate::representation_plan::ProducerCarrierPlan::build(&func),
        );
        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        backend.module.verify().unwrap();
        let ir = llvm_fn.print_to_string().to_string();
        assert!(
            ir.contains_execution("ret i64 %0"),
            "object bits must survive: {ir}"
        );
        assert!(
            ir.contains_execution("@__molt_inline_retain(i64 %0,"),
            "{ir}"
        );
        assert!(
            ir.contains_execution("@__molt_inline_release(i64 %0,"),
            "{ir}"
        );
        assert!(!ir.contains_execution("bitcast i64"), "{ir}");
        assert!(!ir.contains_execution("trunc i64"), "{ir}");
        assert!(!ir.contains_execution("@molt_int_from_i64"), "{ir}");
    }
}

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
        let func = TirFunction::new(
            "rc_carriers".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        let llvm_fn = backend.module.add_compiled_function(
            "rc_carriers",
            ctx.void_type().fn_type(&[], false),
            None,
        );
        backend
            .builder
            .position_at_end(ctx.append_basic_block(llvm_fn, "entry"));
        let mut lowering = make_dummy_lowering(&backend, &func, llvm_fn);
        let value: BasicValueEnum<'_> = match ty {
            TirType::F64 => ctx.f64_type().const_float(-0.0).into(),
            TirType::Bool => ctx.bool_type().const_int(1, false).into(),
            _ => ctx.i64_type().const_int(i64::MAX as u64, false).into(),
        };
        lowering.values.set_value(ValueId(0), value);
        lowering.values.set_type(ValueId(0), ty.clone());
        for opcode in [OpCode::IncRef, OpCode::DecRef, OpCode::DelBoundary] {
            lowering.lower_op(
                func.entry_block,
                &TirOp {
                    origin: None,
                    runtime: None,
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
        assert_eq!(
            ir.matches_execution("call void @__molt_inline_").count(),
            expected,
            "{ty:?}: {ir}"
        );
        assert!(
            !ir.contains_execution("@molt_int_from_i64"),
            "RC must never allocate: {ir}"
        );
    }
}

#[test]
fn box_and_reference_unbox_each_mint_their_own_heap_owner() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let func = TirFunction::new(
        "owned_box".into(),
        vec![TirType::DynBox],
        TirType::Str,
        molt_ir::FunctionReturnAbi::Value,
    );
    let llvm_fn = backend.module.add_compiled_function(
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
        .set_value(ValueId(0), llvm_fn.get_nth_param(1).unwrap());
    lowering.values.set_type(ValueId(0), TirType::DynBox);
    let mut op = TirOp {
        origin: None,
        runtime: None,
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
    lowering.values.set_type(ValueId(2), TirType::Str);
    lowering.emit_unbox(&op);
    backend
        .builder
        .build_return(Some(&lowering.resolve(ValueId(2))))
        .unwrap();
    backend.module.verify().unwrap();
    let ir = llvm_fn.print_to_string().to_string();
    assert_eq!(
        ir.matches_execution("call void @__molt_inline_retain")
            .count(),
        2,
        "{ir}"
    );
}

#[test]
fn discarded_representation_results_preserve_only_boxing_allocation_effects() {
    for (opcode, ty, inline) in [
        (OpCode::BoxVal, TirType::I64, false),
        (OpCode::BoxVal, TirType::I64, true),
        (OpCode::BoxVal, TirType::DynBox, false),
        (OpCode::UnboxVal, TirType::DynBox, false),
    ] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let func = TirFunction::new(
            "discarded_representation".into(),
            vec![ty.clone()],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        let llvm_fn = backend.module.add_compiled_function(
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
            .set_value(ValueId(0), llvm_fn.get_nth_param(1).unwrap());
        lowering.values.set_type(ValueId(0), ty.clone());
        if inline {
            lowering
                .repr_facts
                .repr_by_value
                .insert(ValueId(0), molt_ir::repr::Repr::RawI64Safe);
        }
        lowering.lower_op(
            func.entry_block,
            &TirOp {
                origin: None,
                runtime: None,
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
        let allocates = opcode == OpCode::BoxVal && ty == TirType::I64 && !inline;
        assert_eq!(
            ir.matches_execution("call i64 @molt_int_from_i64").count(),
            usize::from(allocates),
            "{ir}"
        );
        assert_eq!(
            ir.matches_execution("call void @__molt_inline_release")
                .count(),
            usize::from(allocates),
            "{ir}"
        );
        assert!(
            !ir.contains_execution("call void @__molt_inline_retain"),
            "{ir}"
        );
        assert!(!ir.contains_execution("call i64 @molt_int_as_i64"), "{ir}");
    }
}

#[test]
fn integer_unbox_and_trampoline_share_full_width_fixed_block_decoder() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let func = TirFunction::new(
        "decode_i64".into(),
        vec![TirType::DynBox],
        TirType::I64,
        molt_ir::FunctionReturnAbi::Value,
    );
    let llvm_fn = backend.module.add_compiled_function(
        "decode_i64",
        ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
        None,
    );
    let entry = ctx.append_basic_block(llvm_fn, "entry");
    backend.builder.position_at_end(entry);
    let mut lowering = make_dummy_lowering(&backend, &func, llvm_fn);
    let raw = llvm_fn.get_nth_param(1).unwrap().into_int_value();
    lowering.values.set_value(ValueId(0), raw.into());
    lowering.values.set_type(ValueId(0), TirType::DynBox);
    lowering.values.set_type(ValueId(1), TirType::I64);
    // Lowerer-only control: the caller guarantees a full-width integer
    // extraction, rather than merely annotating an arbitrary Python object.
    lowering
        .repr_facts
        .repr_by_value
        .insert(ValueId(1), molt_ir::repr::Repr::RawI64FullDeopt);
    lowering.emit_unbox(&TirOp {
        origin: None,
        runtime: None,
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
    assert_eq!(
        ir.matches_execution("call i64 @molt_int_as_i64").count(),
        2,
        "{ir}"
    );
    assert_eq!(
        llvm_fn.count_basic_blocks(),
        1,
        "phi insertions must remain in their predecessor"
    );
    assert!(
        !ir.contains_execution("sign_extend") && !ir.contains_execution("payload"),
        "{ir}"
    );
}

#[test]
fn heap_literals_materialize_an_owned_result_after_an_equal_result_is_dropped() {
    for (opcode, result_ty, runtime_name, payload) in [
        (
            OpCode::ConstStr,
            TirType::Str,
            RuntimeFn::StringFromBytes,
            AttrValue::Bytes(b"same\0literal".to_vec()),
        ),
        (
            OpCode::ConstBytes,
            TirType::Bytes,
            RuntimeFn::BytesFromBytes,
            AttrValue::Bytes(vec![0xff, 0, 0x80]),
        ),
        (
            OpCode::ConstBigInt,
            TirType::DynBox,
            RuntimeFn::BigintFromBytes,
            AttrValue::Bytes(vec![
                0x01, 0x8e, 0xe9, 0x0f, 0xf6, 0xc3, 0x73, 0xe0, 0xee, 0x4e, 0x3f, 0x0a, 0xd2,
            ]),
        ),
    ] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let func = TirFunction::new(
            "literal_ownership".into(),
            vec![],
            result_ty.clone(),
            molt_ir::FunctionReturnAbi::Value,
        );
        let llvm_fn = backend.module.add_compiled_function(
            "literal_ownership",
            ctx.i64_type().fn_type(&[], false),
            None,
        );
        let entry = ctx.append_basic_block(llvm_fn, "entry");
        backend.builder.position_at_end(entry);
        let mut lowering = make_dummy_lowering(&backend, &func, llvm_fn);
        let mut attrs = AttrDict::new();
        attrs.insert("bytes".into(), payload);
        for result in [ValueId(0), ValueId(1)] {
            lowering.lower_op(
                func.entry_block,
                &TirOp {
                    origin: None,
                    runtime: None,
                    dialect: Dialect::Molt,
                    opcode,
                    operands: vec![],
                    results: vec![result],
                    attrs: attrs.clone(),
                    source_span: None,
                },
            );
            assert_eq!(*lowering.values.ty(result).unwrap(), result_ty);
            if result == ValueId(0) {
                lowering.lower_op(
                    func.entry_block,
                    &TirOp {
                        origin: None,
                        runtime: None,
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
        assert_ne!(
            lowering.values.value(ValueId(0)).unwrap(),
            lowering.values.value(ValueId(1)).unwrap()
        );
        let result = lowering.ensure_i64(lowering.values.value(ValueId(1)).unwrap());
        backend.builder.build_return(Some(&result)).unwrap();
        assert!(lowering.diagnostics.borrow().is_empty());
        backend.module.verify().expect("literal module verifies");
        let ir = llvm_fn.print_to_string().to_string();
        let calls: Vec<_> = ir
            .lines()
            .filter(|line| {
                line.contains_execution("call ")
                    && !line.contains_execution("@molt_refcount_admission(")
            })
            .collect();
        assert_eq!(
            calls.len(),
            3,
            "one owned call per result and one drop: {ir}"
        );
        assert!(
            calls[0].contains_execution(&format!("@{}(", runtime_name.symbol())),
            "{ir}"
        );
        assert!(
            calls[1].contains_execution("@__molt_inline_release("),
            "{ir}"
        );
        assert!(
            calls[2].contains_execution(&format!("@{}(", runtime_name.symbol())),
            "{ir}"
        );
        let materializer = backend
            .module
            .get_function(runtime_name.linker_symbol())
            .unwrap();
        assert_eq!(
            materializer.get_type(),
            crate::llvm_backend::runtime_imports::runtime_function(
                &ctx,
                &backend.module,
                runtime_name
            )
            .get_type(),
            "{}",
            runtime_name.symbol()
        );
    }
}

#[test]
fn repeated_attribute_name_literals_use_owned_string_materialization() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let func = TirFunction::new(
        "literal_names".into(),
        vec![],
        TirType::Str,
        molt_ir::FunctionReturnAbi::Value,
    );
    let llvm_fn = backend.module.add_compiled_function(
        "literal_names",
        ctx.i64_type().fn_type(&[], false),
        None,
    );
    let entry = ctx.append_basic_block(llvm_fn, "entry");
    backend.builder.position_at_end(entry);
    let mut lowering = make_dummy_lowering(&backend, &func, llvm_fn);
    let mut results = Vec::new();
    for _ in 0..2 {
        results.push(lowering.with_owned_name("same_name", |this, name_bits| {
            let import = this.ensure_runtime_import(RuntimeFn::ModuleImport);
            this.backend
                .builder
                .build_execution_call(import, &[name_bits.into()], "name_consumer")
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
    assert_eq!(
        ir.matches_execution("@molt_string_from_bytes(").count(),
        2,
        "{ir}"
    );
    assert_eq!(
        ir.matches_execution("@__molt_inline_release(").count(),
        2,
        "{ir}"
    );
    assert_eq!(ir.matches_execution("icmp eq i32").count(), 2, "{ir}");
    assert!(!ir.contains_execution("@molt_bytes_from_bytes("), "{ir}");
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
        let mut func = TirFunction::new(
            "checked_name".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let first = func.fresh_value();
        let second = func.fresh_value();
        let result_id = func.fresh_value();
        let handler_result = func.fresh_value();
        let handler_id = func.fresh_block();
        func.has_exception_handling = true;
        func.label_id_map.insert(handler_id.0, 17);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend([const_none_def(first), const_none_def(second)]);
        entry.ops.push(TirOp {
            origin: None,
            runtime: None,
            dialect: Dialect::Molt,
            opcode,
            operands: [first, second].into_iter().take(operands).collect(),
            results: if has_result { vec![result_id] } else { vec![] },
            attrs: AttrDict::from([(key.into(), AttrValue::Str("same_name".into()))]),
            source_span: None,
        });
        // The enclosing operation, not a private return, owns exception routing.
        let mut check = TirOp {
            origin: None,
            runtime: None,
            dialect: Dialect::Molt,
            opcode: OpCode::CheckException,
            operands: vec![],
            results: vec![],
            attrs: AttrDict::from([("value".into(), AttrValue::Int(17))]),
            source_span: None,
        };
        check.set_exception_edge_kind(ExceptionEdgeKind::Origin);
        entry.ops.push(check);
        entry.terminator = Terminator::Return {
            values: vec![if has_result { result_id } else { first }],
        };
        func.blocks.insert(
            handler_id,
            TirBlock {
                id: handler_id,
                args: vec![],
                ops: vec![const_none_def(handler_result)],
                terminator: Terminator::Return {
                    values: vec![handler_result],
                },
            },
        );
        let llvm_fn = try_lower_tir_to_llvm(&func, &backend)
            .expect("checked-name consumer must lower with its explicit exception successor");
        backend
            .module
            .verify()
            .expect("checked name CFG must verify");
        let ir = llvm_fn.print_to_string().to_string();
        let entry_name = format!("bb{}", func.entry_block.0);
        let source = block_text(
            llvm_fn
                .get_basic_blocks()
                .into_iter()
                .find(|block| block.get_name().to_bytes() == entry_name.as_bytes())
                .unwrap(),
        );
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
        assert!(source.contains_execution("icmp eq i32 %sfb, 0"), "{ir}");
        assert!(
            source.contains_execution(
                "br i1 %owned_name_ok, label %owned_name_success0, label %owned_name_merge0"
            ),
            "{ir}"
        );
        assert!(!source.contains_execution(&format!("@{runtime}(")), "{ir}");
        assert!(
            !source.contains_execution("@__molt_inline_release("),
            "{ir}"
        );
        assert_eq!(
            success.matches_execution(&format!("@{runtime}(")).count(),
            1,
            "{ir}"
        );
        assert_eq!(
            ir.matches_execution("@__molt_inline_release(i64 %str_bits, i64 %rc_admission)")
                .count(),
            1,
            "{ir}"
        );
        assert!(
            success.find_execution(&format!("@{runtime}(")).unwrap()
                < success.find_execution("@__molt_inline_release(").unwrap(),
            "{ir}"
        );
        assert!(!success.contains_execution("ret "), "{ir}");
        assert!(
            merge.contains_execution(&format!(
                "[ {}, %{entry_name} ]",
                nanbox::QNAN | nanbox::TAG_NONE
            )),
            "{ir}"
        );
        // Invariant J1: the merge tests the thread record's pending byte
        // inline; only a set byte reaches the runtime reader, and the join
        // selects the exceptional edge from the confirmed answer.
        let named = |prefix: &str| {
            block_text(
                llvm_fn
                    .get_basic_blocks()
                    .into_iter()
                    .find(|block| block.get_name().to_str().unwrap().starts_with(prefix))
                    .unwrap_or_else(|| panic!("{prefix} block: {ir}")),
            )
        };
        let confirm = named("exception_pending_confirm");
        let join = named("exception_pending_join");
        assert!(
            merge.contains_execution("load i8, ptr %check_exc_byte_ptr"),
            "{ir}"
        );
        assert!(!merge.contains_execution("@molt_exception_pending"), "{ir}");
        assert!(
            confirm.contains_execution("@molt_exception_pending"),
            "{ir}"
        );
        assert!(
            join.contains_execution("icmp ne i64 %check_exc_word, 0"),
            "{ir}"
        );
        let origin = llvm_fn
            .get_basic_blocks()
            .into_iter()
            .find(|block| block_text(*block).contains_execution("@molt_exception_frame_origin("))
            .expect("the operation's taken Origin edge captures before the handler");
        let origin_name = origin.get_name().to_str().unwrap();
        let origin_text = block_text(origin);
        assert!(
            join.contains_execution(&format!(
                "br i1 %check_exc_pending, label %{origin_name}, label %check_exc_cont",
            )),
            "{ir}"
        );
        assert_eq!(
            ir.matches_execution("@molt_exception_frame_origin(")
                .count(),
            1,
            "{ir}"
        );
        assert!(
            origin_text.contains_execution(&format!("br label %bb{}", handler_id.0)),
            "{ir}"
        );
        for normal_path in [&source, &success, &merge] {
            assert!(
                !normal_path.contains_execution("@molt_exception_frame_origin("),
                "{ir}"
            );
        }
        assert!(!merge.contains_execution("ret "), "{ir}");
        for block_name in [format!("bb{}", handler_id.0), "check_exc_cont".into()] {
            let destination = block_text(
                llvm_fn
                    .get_basic_blocks()
                    .into_iter()
                    .find(|block| {
                        block
                            .get_name()
                            .to_bytes()
                            .starts_with(block_name.as_bytes())
                    })
                    .unwrap(),
            );
            assert!(destination.contains_execution("ret i64"), "{ir}");
            assert!(
                !destination.contains_execution(&format!("@{runtime}(")),
                "{ir}"
            );
            assert!(
                !destination.contains_execution("@__molt_inline_release("),
                "{ir}"
            );
            assert!(
                !destination.contains_execution("@molt_exception_frame_origin("),
                "{ir}"
            );
        }
        assert!(
            !merge.contains_execution("@__molt_inline_release(")
                && !merge.contains_execution("ptr_unbox"),
            "{ir}"
        );
        if opcode == OpCode::CallBuiltin {
            assert!(!source.contains_execution("@molt_callargs_new("), "{ir}");
            assert!(success.contains_execution("@molt_callargs_new("), "{ir}");
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
    result_repr: Option<molt_ir::repr::Repr>,
) -> (String, TirType) {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let func = TirFunction::new(
        "unary_carrier".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let llvm_fn = backend.module.add_compiled_function(
        "unary_carrier",
        ctx.i64_type().fn_type(&[], false),
        None,
    );
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
    lowering.values.set_value(operand, value);
    lowering.values.set_type(operand, operand_ty);
    if let Some(repr) = result_repr {
        lowering.repr_facts.repr_by_value.insert(result, repr);
    }
    lowering.lower_op(
        func.entry_block,
        &TirOp {
            origin: None,
            runtime: None,
            dialect: Dialect::Molt,
            opcode,
            operands: vec![operand],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        },
    );
    let result_ty = lowering.values.ty(result).unwrap().clone();
    let bits = lowering.ensure_i64(lowering.values.value(result).unwrap());
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
                ir.contains_execution(&format!("@{symbol}(i64 {boxed})")),
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
                ir.contains_execution(&format!("@{symbol}(i64 {boxed})")),
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
        assert!(
            ir.contains_execution(&format!("@{symbol}(i64 {bits})")),
            "{ir}"
        );
        assert_eq!(result_ty, TirType::DynBox, "{ir}");
    }
}

#[test]
fn unary_neg_without_result_range_proof_boxes_full_i64_before_runtime() {
    for value in [i64::MIN, i64::MAX, -(1_i64 << 46), 42] {
        let (ir, result_ty) =
            lower_unary_carrier_fixture(OpCode::Neg, TirType::I64, value as u64, None);
        assert!(
            ir.contains_execution("@molt_neg(i64 "),
            "unproved neg must use runtime: {ir}"
        );
        assert!(
            ir.contains_execution(&format!("@molt_int_from_i64(i64 {value})")),
            "fallback must preserve the full i64 before runtime dispatch: {ir}"
        );
        assert!(
            !ir.contains_execution(&format!("@molt_neg(i64 {value})")),
            "raw integer bits are not boxed runtime operands: {ir}"
        );
        assert_eq!(result_ty, TirType::DynBox, "{ir}");
    }
}

#[test]
fn binary_runtime_fallbacks_share_one_box_for_a_repeated_raw_operand() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "binary_owner".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let raw = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_int_def(raw, i64::MAX));
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![raw, raw],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    backend.module.verify().expect("boxed binary fallback");
    let ir = llvm_fn.print_to_string().to_string();
    let call = "call i64 @molt_add(i64 %boxed_int, i64 %boxed_int)";
    let release = "call void @__molt_inline_release(i64 %binary_owner_bits, i64 %rc_admission)";
    assert!(
        ir.contains_execution(call),
        "both positions carry one box: {ir}"
    );
    assert_eq!(
        ir.matches_execution("call i64 @molt_int_from_i64(").count(),
        1,
        "{ir}"
    );
    assert_eq!(ir.matches_execution(release).count(), 1, "{ir}");
    assert!(
        ir.find_execution(call).unwrap() < ir.find_execution(release).unwrap(),
        "the runtime borrows the box before it is released: {ir}"
    );
}

#[test]
fn boolean_selection_owns_exactly_the_selected_operand() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "select_scalar".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let raw = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let object = entry.args[0].id;
    entry.ops.push(const_int_def(raw, i64::MAX));
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Or,
        operands: vec![raw, object],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    backend.module.verify().expect("scalar boolean selection");
    let ir = llvm_fn.print_to_string().to_string();
    let block_text = |prefix: &str| -> String {
        let block = llvm_fn
            .get_basic_blocks()
            .into_iter()
            .find(|block| block.get_name().to_str().unwrap().starts_with(prefix))
            .unwrap_or_else(|| panic!("missing {prefix}: {ir}"));
        let mut text = String::new();
        let mut instruction = block.get_first_instruction();
        while let Some(current) = instruction {
            text.push_str(&current.print_to_string().to_string());
            text.push('\n');
            instruction = current.get_next_instruction();
        }
        text
    };
    assert!(
        !ir.contains_execution("@molt_is_truthy("),
        "a raw integer's truthiness needs no runtime call: {ir}"
    );
    assert!(
        block_text("bool_or_true").contains_execution("label %box_int_inline"),
        "only the edge that selects the raw integer boxes it: {ir}"
    );
    assert_eq!(
        ir.matches_execution("call i64 @molt_int_from_i64(").count(),
        1,
        "{ir}"
    );
    let object_edge = block_text("bool_or_false");
    assert!(
        object_edge
            .contains_execution("call void @__molt_inline_retain(i64 %0, i64 %rc_admission)")
            && !object_edge.contains_execution("@molt_int_from_i64("),
        "the selected object is retained and the unselected integer is not boxed: {ir}"
    );
    assert_eq!(
        ir.matches_execution("call void @__molt_inline_retain(")
            .count(),
        1,
        "{ir}"
    );
    assert!(
        !ir.contains_execution("@__molt_inline_release("),
        "a minted box is the selected result's owner: {ir}"
    );
    assert!(ir.contains_execution("%bool_or = phi i64"), "{ir}");
}

#[test]
fn not_in_releases_the_owned_membership_result() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "not_in_owner".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::Bool,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let operands = entry.args.iter().map(|arg| arg.id).collect();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::NotIn,
        operands,
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    backend.module.verify().expect("negated membership");
    let ir = llvm_fn.print_to_string().to_string();
    let contains = ir
        .find_execution("%molt_contains = call i64 @molt_contains(i64 %0, i64 %1)")
        .unwrap_or_else(|| panic!("{ir}"));
    let truthy = ir
        .find_execution("call i64 @molt_is_truthy(i64 %molt_contains)")
        .unwrap_or_else(|| panic!("{ir}"));
    let release = ir
        .find_execution("call void @__molt_inline_release(i64 %molt_contains, i64 %rc_admission)")
        .unwrap_or_else(|| panic!("only its truthiness escapes: {ir}"));
    assert!(contains < truthy && truthy < release, "{ir}");
}

#[test]
fn unary_proven_numeric_lanes_keep_raw_results() {
    for (opcode, operand, expected, result_repr) in [
        (
            OpCode::Neg,
            42_i64,
            -42_i64,
            Some(molt_ir::repr::Repr::RawI64Safe),
        ),
        (OpCode::Pos, i64::MIN, i64::MIN, None),
        (OpCode::BitNot, i64::MIN, i64::MAX, None),
    ] {
        let (ir, result_ty) =
            lower_unary_carrier_fixture(opcode, TirType::I64, operand as u64, result_repr);
        assert!(
            ir.contains_execution(&format!("ret i64 {expected}")),
            "{ir}"
        );
        assert!(
            !ir.contains_execution("call "),
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
                ir.contains_execution(&format!("ret i64 {}", expected.to_bits() as i64)),
                "float unary must preserve the sign of zero: {ir}"
            );
            assert!(!ir.contains_execution("call "), "{ir}");
            assert_eq!(result_ty, TirType::F64);
        }
    }
    for value in [false, true] {
        let (ir, result_ty) =
            lower_unary_carrier_fixture(OpCode::Not, TirType::Bool, u64::from(value), None);
        assert!(
            ir.contains_execution(&format!("ret i64 {}", u64::from(!value))),
            "{ir}"
        );
        assert!(!ir.contains_execution("call "), "{ir}");
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
        molt_ir::FunctionReturnAbi::Value,
    );
    let v_result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
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
    let mut facts = crate::representation_plan::ProducerCarrierPlan::default();
    for v in [ValueId(0), ValueId(1)] {
        facts
            .repr_by_value
            .insert(v, molt_ir::repr::Repr::RawI64Safe);
    }
    backend.function_repr_facts.insert(func.name.clone(), facts);

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(
        ir.contains_execution("icmp slt"),
        "expected signed less-than comparison in IR: {}",
        ir
    );
}

#[test]
fn lower_box_i64() {
    for transfer in ["box", "return", "phi"] {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);

        // Build: fn box_it(x: i64) -> DynBox { return box(x) }
        let mut func = TirFunction::new(
            "box_i64".into(),
            vec![TirType::I64],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let v_boxed = func.fresh_value();
        let target = func.fresh_block();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        if transfer == "box" {
            entry.ops.push(TirOp {
                origin: None,
                runtime: None,
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
        } else if transfer == "return" {
            entry.terminator = Terminator::Return {
                values: vec![ValueId(0)],
            };
        } else {
            entry.terminator = Terminator::Branch {
                target,
                args: vec![ValueId(0)],
            };
            func.blocks.insert(
                target,
                TirBlock {
                    id: target,
                    args: vec![TirValue {
                        id: v_boxed,
                        ty: TirType::DynBox,
                    }],
                    ops: vec![],
                    terminator: Terminator::Return {
                        values: vec![v_boxed],
                    },
                },
            );
        }

        // `box(x)` emits the NaN-boxing arithmetic only when `x` is a RAW i64.
        // An unproven `i64` parameter carries already-boxed (`DynBox`), for which
        // `box` is a no-op; prove the parameter raw so the box path is exercised.
        let mut facts = crate::representation_plan::ProducerCarrierPlan::default();
        facts
            .repr_by_value
            .insert(ValueId(0), molt_ir::repr::Repr::RawI64Safe);
        backend.function_repr_facts.insert(func.name.clone(), facts);

        // This test owns an explicitly boxed destination ABI. Inferred
        // producer-proven raw returns are tested separately by linkage tests.
        backend.function_linkage_abis.insert(
            func.name.clone(),
            test_native_linkage_abi(vec![TirType::I64], Some(TirType::DynBox)),
        );
        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        backend.module.verify().unwrap();
        let ir = llvm_fn.print_to_string().to_string();
        assert!(
            !ir.contains_execution("@molt_int_from_i64"),
            "{transfer}: {ir}"
        );
        assert!(!ir.contains_execution("br i1"), "{transfer}: {ir}");

        // Should contain the NaN-boxing OR operations
        assert!(
            ir.contains_execution("or i64"),
            "expected NaN-boxing OR in IR: {}",
            ir
        );
        assert!(
            ir.contains_execution("and i64"),
            "expected NaN-boxing AND mask in IR: {}",
            ir
        );
    }
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

    let mut func = TirFunction::new(
        "masked_shift".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
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
        origin: None,
        runtime: None,
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
        origin: None,
        runtime: None,
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
    let mut facts = crate::representation_plan::ProducerCarrierPlan::default();
    for v in [s_phi, shl, band] {
        facts
            .repr_by_value
            .insert(v, molt_ir::repr::Repr::RawI64Safe);
    }
    backend.function_repr_facts.insert(func.name.clone(), facts);

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();

    assert!(
        ir.contains_execution("shl i64"),
        "masked accumulator shift must lower to a RAW machine `shl i64`, not \
             the boxed runtime. IR:\n{ir}"
    );
    assert!(
        !ir.contains_execution("@molt_lshift"),
        "a RawI64Safe-proven masked shift must NOT call the boxed `molt_lshift`. \
             IR:\n{ir}"
    );
    assert!(
        !ir.contains_execution("@molt_bit_and"),
        "a RawI64Safe-proven masked `& MASK` must NOT call the boxed \
             `molt_bit_and`. IR:\n{ir}"
    );
    // The header phi must be a raw `i64` phi (promoted from its DynBox
    // declaration) so the back-edge carries the raw masked value.
    assert!(
        ir.contains_execution("phi i64"),
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
        let mut function = TirFunction::new(
            "__molt_chunk_v1_user".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
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

#[test]
fn proven_none_boxing_aliases_and_refcounts_emit_no_transition() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "proven_immortal_rc".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let none = func.fresh_value();
    let boxed = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(none));
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::BoxVal,
        operands: vec![none],
        results: vec![boxed],
        attrs: AttrDict::new(),
        source_span: None,
    });
    for opcode in [OpCode::IncRef, OpCode::DecRef] {
        entry.ops.push(TirOp {
            origin: None,
            runtime: None,
            dialect: Dialect::Molt,
            opcode,
            operands: vec![boxed],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
    }
    entry.terminator = Terminator::Return { values: vec![] };
    backend.function_repr_facts.insert(
        func.name.clone(),
        crate::representation_plan::ProducerCarrierPlan::build(&func),
    );
    let function = lower_tir_to_llvm(&func, &backend);
    backend.module.verify().unwrap();
    let ir = function.print_to_string().to_string();
    assert!(
        !ir.contains_execution("__molt_inline_")
            && !ir.contains_execution("molt_refcount_admission"),
        "producer-proven None has no heap operation: {ir}"
    );
}

#[test]
fn identity_uses_canonical_object_words() {
    // Each pair denotes the same Python singleton/object in two physical
    // carriers, or distinct boolean/integer/float singleton classes. Expected
    // identity follows canonical object words, independently of LLVM lowering.
    let boxed_false = nanbox::QNAN | nanbox::TAG_BOOL;
    let boxed_true = boxed_false | 1;
    let none = nanbox::QNAN | nanbox::TAG_NONE;
    let cases = [
        (TirType::DynBox, boxed_false, TirType::Bool, 0, true),
        (TirType::DynBox, boxed_true, TirType::Bool, 1, true),
        (TirType::DynBox, boxed_true, TirType::Bool, 0, false),
        (TirType::Bool, 0, TirType::I64, 0, false),
        (TirType::Bool, 1, TirType::I64, 1, false),
        (
            TirType::DynBox,
            nanbox::QNAN | nanbox::TAG_INT | 1,
            TirType::I64,
            1,
            true,
        ),
        (
            TirType::DynBox,
            1.25_f64.to_bits(),
            TirType::F64,
            1.25_f64.to_bits(),
            true,
        ),
        (TirType::DynBox, none, TirType::None, none, true),
        (TirType::DynBox, boxed_false, TirType::None, none, false),
    ];
    for (left_ty, left_bits, right_ty, right_bits, expected_same) in cases {
        for swapped in [false, true] {
            for opcode in [OpCode::Is, OpCode::IsNot] {
                let ctx = Context::create();
                let backend = make_backend(&ctx);
                let func = TirFunction::new(
                    "identity_words".into(),
                    vec![],
                    TirType::Bool,
                    molt_ir::FunctionReturnAbi::Value,
                );
                let llvm_fn = backend.module.add_compiled_function(
                    "identity_words",
                    ctx.i64_type().fn_type(&[], false),
                    None,
                );
                backend
                    .builder
                    .position_at_end(ctx.append_basic_block(llvm_fn, "entry"));
                let mut lowering = make_dummy_lowering(&backend, &func, llvm_fn);
                for (id, ty, bits) in [
                    (ValueId(0), left_ty.clone(), left_bits),
                    (ValueId(1), right_ty.clone(), right_bits),
                ] {
                    let value: BasicValueEnum<'_> = match ty {
                        TirType::Bool => ctx.bool_type().const_int(bits, false).into(),
                        TirType::F64 => ctx.f64_type().const_float(f64::from_bits(bits)).into(),
                        _ => ctx.i64_type().const_int(bits, false).into(),
                    };
                    if ty == TirType::I64 {
                        lowering
                            .repr_facts
                            .repr_by_value
                            .insert(id, molt_ir::repr::Repr::RawI64Safe);
                    }
                    lowering.values.set_value(id, value);
                    lowering.values.set_type(id, ty);
                }
                let result = ValueId(2);
                lowering.lower_op(
                    func.entry_block,
                    &TirOp {
                        origin: None,
                        runtime: None,
                        dialect: Dialect::Molt,
                        opcode,
                        operands: if swapped {
                            vec![ValueId(1), ValueId(0)]
                        } else {
                            vec![ValueId(0), ValueId(1)]
                        },
                        results: vec![result],
                        attrs: AttrDict::new(),
                        source_span: None,
                    },
                );
                assert!(lowering.diagnostics.borrow().is_empty());
                assert_eq!(*lowering.values.ty(result).unwrap(), TirType::Bool);
                let actual = lowering
                    .values
                    .value(result)
                    .unwrap()
                    .into_int_value()
                    .get_zero_extended_constant();
                let expected = expected_same ^ (opcode == OpCode::IsNot);
                assert_eq!(
                    actual,
                    Some(u64::from(expected)),
                    "{left_ty:?}/{right_ty:?} {opcode:?} swapped={swapped}"
                );
                backend
                    .builder
                    .build_return(Some(
                        &lowering.ensure_i64(lowering.values.value(result).unwrap()),
                    ))
                    .unwrap();
                backend.module.verify().unwrap();
            }
        }
    }
}
