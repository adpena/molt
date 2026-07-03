use super::*;

#[test]
fn boxed_or_retains_selected_operand_result() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "boxed_or_selected_owner".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Or,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_is_truthy"), "{ir}");
    assert!(
        ir.contains("call void @molt_inc_ref_obj(i64 %bool_or)"),
        "{ir}"
    );
}

#[test]
fn removed_runtime_delegates_fail_before_phantom_imports() {
    for &(opcode, dialect, operands, symbol, message) in &[
        (
            OpCode::Yield,
            Dialect::Molt,
            1usize,
            "molt_yield",
            "explicit state-machine poll/resume",
        ),
        (
            OpCode::YieldFrom,
            Dialect::Molt,
            1,
            "molt_yield_from",
            "generator delegation",
        ),
        (OpCode::ScfIf, Dialect::Scf, 1, "molt_call_0", "LLVM CFG"),
        (
            OpCode::ScfFor,
            Dialect::Scf,
            4,
            "molt_scf_for",
            "loops into LLVM CFG",
        ),
        (
            OpCode::ScfWhile,
            Dialect::Scf,
            2,
            "molt_scf_while",
            "while regions",
        ),
        (
            OpCode::ScfYield,
            Dialect::Scf,
            1,
            "molt_scf_yield",
            "phi nodes",
        ),
    ] {
        let (err, backend) =
            lowering_error_for_single_op("removed_runtime_delegate", dialect, opcode, operands);
        assert_lowering_error_contains(&err, symbol);
        assert_lowering_error_contains(&err, message);
        assert!(
            backend.module.get_function(symbol).is_none(),
            "{symbol} must not be declared as a phantom runtime import"
        );
    }
}

#[test]
fn iterator_ops_lower_to_real_runtime_exports() {
    for &(opcode, expected, forbidden, call_name) in &[
        (
            OpCode::GetIter,
            "molt_iter_checked",
            "molt_get_iter",
            "iter_checked",
        ),
        (
            OpCode::ForIter,
            "molt_iter_next",
            "molt_for_iter",
            "for_iter_next",
        ),
    ] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(format!("iterator_{call_name}"), vec![], TirType::DynBox);
        let operand = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(operand));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![operand],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let ir = try_lower_tir_to_llvm(&func, &backend)
            .unwrap()
            .print_to_string()
            .to_string();
        assert!(ir.contains(expected), "{ir}");
        assert!(ir.contains(call_name), "{ir}");
        assert!(!ir.contains(forbidden), "{ir}");
    }
}

#[test]
#[should_panic(expected = "LLVM function type mismatch for `same_name`")]
fn llvm_symbol_signature_mismatch_rejects_tir_forward_declaration() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    backend.module.add_function(
        "same_name",
        ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
        Some(inkwell::module::Linkage::External),
    );
    let func = TirFunction::new("same_name".into(), vec![], TirType::I64);

    let _ = declare_tir_function(&func, &backend);
}

#[test]
#[should_panic(expected = "LLVM function type mismatch for `molt_trace_exit`")]
fn llvm_symbol_signature_mismatch_rejects_runtime_i64_reuse() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "test");
    backend.module.add_function(
        "molt_trace_exit",
        ctx.void_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let dummy = TirFunction::new("dummy_runtime_symbol".into(), vec![], TirType::DynBox);
    let dummy_fn = backend.module.add_function(
        "dummy_runtime_symbol",
        ctx.i64_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

    let _ = lowering.ensure_runtime_i64_fn("molt_trace_exit", 0);
}

#[test]
#[should_panic(expected = "LLVM function type mismatch for `molt_inc_ref_obj`")]
fn llvm_symbol_signature_mismatch_rejects_runtime_void_reuse() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "test");
    backend.module.add_function(
        "molt_inc_ref_obj",
        ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
        Some(inkwell::module::Linkage::External),
    );
    let dummy = TirFunction::new("dummy_runtime_void_symbol".into(), vec![], TirType::DynBox);
    let dummy_fn = backend.module.add_function(
        "dummy_runtime_void_symbol",
        ctx.i64_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

    let _ = lowering.ensure_runtime_void_fn("molt_inc_ref_obj", 1);
}

#[test]
fn on_demand_runtime_declaration_uses_conservative_attributes() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "test");
    let dummy = TirFunction::new("dummy_runtime_attrs".into(), vec![], TirType::DynBox);
    let dummy_fn = backend.module.add_function(
        "dummy_runtime_attrs",
        ctx.i64_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

    let func = lowering.ensure_runtime_i64_fn("molt_abs_builtin", 1);

    assert!(has_fn_attr(func, "nounwind"));
    assert!(
        lacks_fn_attr(func, "willreturn"),
        "ad-hoc runtime declarations must not claim termination"
    );
}

#[test]
#[should_panic(
    expected = "LLVM runtime import `molt_unclassified_runtime_symbol` has no ABI classification"
)]
fn unclassified_runtime_declaration_rejects_new_symbol_drift() {
    let ctx = Context::create();
    let backend = LlvmBackend::new(&ctx, "test");
    let dummy = TirFunction::new("dummy_runtime_reject".into(), vec![], TirType::DynBox);
    let dummy_fn = backend.module.add_function(
        "dummy_runtime_reject",
        ctx.i64_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

    let _ = lowering.ensure_runtime_i64_fn("molt_unclassified_runtime_symbol", 2);
}

#[test]
fn preserved_runtime_call_rejects_name_only_symbol_drift() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_unclassified_runtime_symbol".to_string());

    let err = lower_preserved_kind_ir(&backend, "unclassified_runtime_symbol", 2, true, None)
        .expect_err("name-only preserved runtime symbols must fail before LLVM declaration");
    assert_lowering_error_contains(&err, "has no LLVM ABI classification");
    assert_lowering_error_contains(&err, "molt_unclassified_runtime_symbol");
}

#[test]
#[should_panic(expected = "LLVM function type mismatch for `gen_fn`")]
fn llvm_symbol_signature_mismatch_rejects_function_symbol_reuse() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend.module.add_function(
        "gen_fn",
        ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
        Some(inkwell::module::Linkage::External),
    );
    backend
        .function_param_types
        .insert("gen_fn".to_string(), vec![TirType::DynBox, TirType::DynBox]);
    let dummy = TirFunction::new("dummy_function_symbol".into(), vec![], TirType::DynBox);
    let dummy_fn = backend.module.add_function(
        "dummy_function_symbol",
        ctx.i64_type().fn_type(&[], false),
        Some(inkwell::module::Linkage::External),
    );
    let lowering = make_dummy_lowering(&backend, &dummy, dummy_fn);

    let _ = lowering.ensure_function_symbol("gen_fn", 0, false);
}
