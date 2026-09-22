use super::*;

#[test]
fn direct_runtime_calls_use_classified_boxed_abi() {
    for (symbol, arity) in [
        ("molt_cell_new", 1),
        ("molt_cell_get", 1),
        ("molt_cell_set", 2),
        ("molt_abs_builtin", 1),
        ("molt_math_sin", 1),
        ("molt_cell_eq", 2),
        ("molt_chan_new", 1),
    ] {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend.runtime_callable_symbols.insert(symbol.into());
        let mut func = TirFunction::new(
            "runtime_call".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let raw = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int_def(raw, 7));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![raw; arity],
            results: vec![result],
            attrs: [
                ("_original_kind".into(), AttrValue::Str("call".into())),
                ("s_value".into(), AttrValue::Str(symbol.into())),
            ]
            .into_iter()
            .collect(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let lowered = lower_tir_to_llvm(&func, &backend);
        backend
            .module
            .verify()
            .expect("classified runtime call ABI");
        let ir = lowered.print_to_string().to_string();
        assert!(ir.contains(&format!("call i64 @{symbol}(")), "{ir}");
        assert!(
            !ir.contains(&format!("@{symbol}(i64 7")),
            "raw integer must be boxed: {ir}"
        );
        assert!(!ir.contains("@molt_call_bind"), "{ir}");
    }
}

#[test]
fn direct_boxed_runtime_calls_preserve_void_result_contracts() {
    for (symbol, arity) in [("molt_spawn", 1), ("molt_print_newline", 0)] {
        for with_result in [false, true] {
            let ctx = Context::create();
            let mut backend = make_backend(&ctx);
            backend.runtime_callable_symbols.insert(symbol.into());
            let mut func = TirFunction::new(
                "void_runtime_call".into(),
                vec![],
                TirType::DynBox,
                molt_ir::FunctionReturnAbi::Value,
            );
            let arg = func.fresh_value();
            let result = func.fresh_value();
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_none_def(arg));
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![arg; arity],
                results: if with_result { vec![result] } else { vec![] },
                attrs: [
                    ("_original_kind".into(), AttrValue::Str("call".into())),
                    ("s_value".into(), AttrValue::Str(symbol.into())),
                ]
                .into_iter()
                .collect(),
                source_span: None,
            });
            entry.terminator = Terminator::Return { values: vec![arg] };
            if with_result {
                let error = try_lower_tir_to_llvm(&func, &backend).expect_err("void call result");
                assert_lowering_error_contains(&error, "has result values");
            } else {
                let lowered = lower_tir_to_llvm(&func, &backend);
                let ir = lowered.print_to_string().to_string();
                assert!(ir.contains(&format!("call void @{symbol}(")), "{ir}");
                backend.module.verify().expect("void boxed call ABI");
            }
        }
    }
}

#[test]
fn boxed_runtime_calls_require_selected_runtime_before_materialization() {
    for opcode in [OpCode::Call, OpCode::Copy] {
        for kind in ["cell_new", "spawn"] {
            for bound in [false, true] {
                let ctx = Context::create();
                let mut backend = make_backend(&ctx);
                let symbol = format!("molt_{kind}");
                backend.runtime_callable_symbols.remove(&symbol);
                let mut func = TirFunction::new(
                    "unavailable_runtime".into(),
                    vec![],
                    TirType::DynBox,
                    molt_ir::FunctionReturnAbi::Value,
                );
                let arg = func.fresh_value();
                let none = func.fresh_value();
                let result = func.fresh_value();
                let entry = func.blocks.get_mut(&func.entry_block).unwrap();
                entry.ops.push(const_int_def(arg, i64::MAX));
                entry.ops.push(const_none_def(none));
                entry.ops.push(TirOp {
                    dialect: Dialect::Molt,
                    opcode,
                    operands: vec![arg],
                    results: if bound { vec![result] } else { vec![] },
                    attrs: AttrDict::from([
                        (
                            "_original_kind".into(),
                            AttrValue::Str(
                                if opcode == OpCode::Call { "call" } else { kind }.into(),
                            ),
                        ),
                        ("s_value".into(), AttrValue::Str(symbol.clone())),
                    ]),
                    source_span: None,
                });
                entry.terminator = Terminator::Return { values: vec![none] };
                let error = try_lower_tir_to_llvm(&func, &backend)
                    .expect_err("runtime availability must precede materialization");
                assert_lowering_error_contains(
                    &error,
                    &format!(
                        "boxed runtime symbol `{symbol}` is unavailable in the selected runtime"
                    ),
                );
                assert!(backend.module.get_function(&symbol).is_none());
                let ir = backend.module.print_to_string().to_string();
                assert!(!ir.contains("call i64 @molt_int_from_i64("), "{ir}");
                assert!(!ir.contains("call void @molt_dec_ref_obj("), "{ir}");
            }
        }
    }
}

#[test]
fn direct_runtime_calls_reject_unclassified_arity_and_internal_target() {
    for (symbol, kind, arity) in [
        ("molt_cell_new", "call", 2),
        ("molt_unknown_cell", "call", 1),
        ("molt_cell_get", "call_internal", 1),
        ("molt_int_from_i64", "call", 1),
        ("molt_int_as_i64", "call", 1),
        ("molt_is_truthy", "call", 1),
        ("molt_obj_get_state", "call", 1),
    ] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "invalid_runtime_call".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let arg = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(arg));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![arg; arity],
            results: vec![result],
            attrs: [
                ("_original_kind".into(), AttrValue::Str(kind.into())),
                ("s_value".into(), AttrValue::Str(symbol.into())),
            ]
            .into_iter()
            .collect(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let error = try_lower_tir_to_llvm(&func, &backend).expect_err("unknown ABI must fail");
        assert_lowering_error_contains(&error, "no exact native linkage ABI");
    }
}

#[test]
fn boxed_runtime_calls_retire_unbound_owned_results() {
    for (opcode, kind) in [(OpCode::Call, "call"), (OpCode::Copy, "cell_new")] {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend
            .runtime_callable_symbols
            .insert("molt_cell_new".into());
        let mut func = TirFunction::new(
            "discard_runtime_result".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let arg = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(arg));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![arg],
            results: vec![],
            attrs: [
                ("_original_kind".into(), AttrValue::Str(kind.into())),
                ("s_value".into(), AttrValue::Str("molt_cell_new".into())),
            ]
            .into_iter()
            .collect(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![arg] };
        let lowered = lower_tir_to_llvm(&func, &backend);
        let ir = lowered.print_to_string().to_string();
        assert!(ir.contains("call i64 @molt_cell_new("), "{ir}");
        assert!(
            ir.contains("call void @molt_dec_ref_obj(i64 %molt_cell_new)"),
            "{ir}"
        );
        backend
            .module
            .verify()
            .expect("discarded boxed result ownership");
    }
}

#[test]
fn boxed_runtime_calls_retire_temporary_integer_owners_separately_from_results() {
    for opcode in [OpCode::Call, OpCode::Copy] {
        for (kind, returns_value) in [("cell_new", true), ("spawn", false)] {
            for (value, inline_proven) in
                [(7, false), (7, true), (i64::MAX, false), (i64::MIN, false)]
            {
                for bound in [false, true] {
                    if bound && !returns_value {
                        continue;
                    }
                    let ctx = Context::create();
                    let mut backend = make_backend(&ctx);
                    let symbol = format!("molt_{kind}");
                    backend.runtime_callable_symbols.insert(symbol.clone());
                    let mut func = TirFunction::new(
                        "boxed_argument_owner".into(),
                        vec![],
                        TirType::DynBox,
                        molt_ir::FunctionReturnAbi::Value,
                    );
                    let arg = func.fresh_value();
                    let none = func.fresh_value();
                    let result = func.fresh_value();
                    if inline_proven {
                        let mut facts = crate::representation_plan::LlvmReprFacts::default();
                        facts.repr_by_value.insert(arg, crate::Repr::RawI64Safe);
                        backend.function_repr_facts.insert(func.name.clone(), facts);
                    }
                    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
                    entry.ops.push(const_int_def(arg, value));
                    entry.ops.push(const_none_def(none));
                    entry.ops.push(TirOp {
                        dialect: Dialect::Molt,
                        opcode,
                        operands: vec![arg],
                        results: if bound { vec![result] } else { vec![] },
                        attrs: AttrDict::from([
                            (
                                "_original_kind".into(),
                                AttrValue::Str(
                                    if opcode == OpCode::Call { "call" } else { kind }.into(),
                                ),
                            ),
                            ("s_value".into(), AttrValue::Str(symbol.clone())),
                        ]),
                        source_span: None,
                    });
                    entry.terminator = Terminator::Return {
                        values: vec![if bound { result } else { none }],
                    };
                    let ir = try_lower_tir_to_llvm(&func, &backend)
                        .unwrap_or_else(|error| panic!("{kind}: {:?}", error.diagnostics()))
                        .print_to_string()
                        .to_string();
                    backend
                        .module
                        .verify()
                        .expect("boxed argument owner lifetime");
                    assert!(
                        ir.contains(&format!("call i64 @molt_int_from_i64(i64 {value})")),
                        "{ir}"
                    );
                    let release = "call void @molt_dec_ref_obj(i64 %boxed_int)";
                    assert_eq!(
                        ir.matches(release).count(),
                        usize::from(!inline_proven),
                        "{ir}"
                    );
                    assert_eq!(
                        ir.matches("call void @molt_dec_ref_obj(").count(),
                        usize::from(!inline_proven) + usize::from(returns_value && !bound),
                        "{ir}"
                    );
                    let call = format!(
                        "call {} @{symbol}(",
                        if returns_value { "i64" } else { "void" }
                    );
                    if !inline_proven {
                        assert!(ir.find(&call).unwrap() < ir.find(release).unwrap(), "{ir}");
                    }
                }
            }
        }
    }
}

#[test]
fn callable_dispatch_retires_only_discarded_owned_results() {
    for (opcode, kind, argc, result_name) in [
        (OpCode::Call, "call_func", 1, "call_func_or_bind_phi"),
        (OpCode::Call, "call_function", 1, "call_func_or_bind_phi"),
        (OpCode::Call, "call_bind", 2, "molt_call_bind_ic"),
        (OpCode::Call, "call_indirect", 2, "molt_call_indirect_ic"),
        (OpCode::Call, "call_guarded", 1, "call_func"),
        (OpCode::Call, "call", 1, "call_result"),
        (OpCode::CallMethod, "call_method", 1, "call_method_bind"),
        (
            OpCode::CallMethodIc,
            "call_method_ic",
            1,
            "molt_call_method_ic0",
        ),
        (
            OpCode::CallSuperMethodIc,
            "call_super_method_ic",
            2,
            "molt_call_super_method_ic0",
        ),
        (OpCode::CallBuiltin, "call_builtin", 2, "call_builtin"),
        (OpCode::CallBuiltin, "named_builtin", 1, "owned_name_result"),
        (OpCode::CallBuiltin, "range_new", 3, "range_new"),
    ] {
        for bound in [false, true] {
            let ctx = Context::create();
            let backend = make_backend(&ctx);
            let mut func = TirFunction::new(
                "call_result_owner".into(),
                vec![],
                TirType::DynBox,
                molt_ir::FunctionReturnAbi::Value,
            );
            let input = func.fresh_value();
            let result = func.fresh_value();
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_none_def(input));
            let mut attrs = AttrDict::from([("method".into(), AttrValue::Str("m".into()))]);
            // Generic CallBuiltin has no preserved source-kind metadata;
            // only specialized primitives such as range_new retain it.
            if !matches!(kind, "call_builtin" | "named_builtin") {
                attrs.insert("_original_kind".into(), AttrValue::Str(kind.into()));
            }
            if kind == "named_builtin" {
                attrs.insert("name".into(), AttrValue::Str("len".into()));
            }
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode,
                operands: vec![input; argc],
                results: if bound { vec![result] } else { vec![] },
                attrs,
                source_span: None,
            });
            entry.terminator = Terminator::Return {
                values: vec![if bound { result } else { input }],
            };
            let ir = try_lower_tir_to_llvm(&func, &backend)
                .unwrap_or_else(|error| panic!("{kind}: {:?}", error.diagnostics()))
                .print_to_string()
                .to_string();
            backend
                .module
                .verify()
                .unwrap_or_else(|error| panic!("{kind}: {error}"));
            let release = format!("call void @molt_dec_ref_obj(i64 %{result_name})");
            assert_eq!(
                ir.matches(&release).count(),
                usize::from(!bound),
                "{kind}, bound={bound}: {ir}"
            );
        }
    }
}

#[test]
fn compiled_call_results_use_semantic_ownership_without_boxing_discarded_scalars() {
    for kind in ["call", "call_internal"] {
        for return_type in [
            None,
            Some(TirType::DynBox),
            Some(TirType::Str),
            Some(TirType::BigInt),
            Some(TirType::I64),
            Some(TirType::F64),
            Some(TirType::Bool),
            Some(TirType::None),
        ] {
            for bound in [false, true] {
                let ctx = Context::create();
                let mut backend = make_backend(&ctx);
                backend.function_linkage_abis.insert(
                    "compiled_result_target".into(),
                    test_native_linkage_abi(vec![], return_type.clone()),
                );
                let mut func = TirFunction::new(
                    "compiled_result_owner".into(),
                    vec![],
                    TirType::DynBox,
                    molt_ir::FunctionReturnAbi::Value,
                );
                let input = func.fresh_value();
                let result = func.fresh_value();
                let entry = func.blocks.get_mut(&func.entry_block).unwrap();
                entry.ops.push(const_none_def(input));
                entry.ops.push(TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Call,
                    operands: vec![],
                    results: if bound { vec![result] } else { vec![] },
                    attrs: AttrDict::from([
                        ("_original_kind".into(), AttrValue::Str(kind.into())),
                        (
                            "s_value".into(),
                            AttrValue::Str("compiled_result_target".into()),
                        ),
                    ]),
                    source_span: None,
                });
                entry.terminator = Terminator::Return {
                    values: vec![if bound { result } else { input }],
                };
                let ir = try_lower_tir_to_llvm(&func, &backend)
                    .unwrap_or_else(|error| {
                        panic!("{kind}, {return_type:?}: {:?}", error.diagnostics())
                    })
                    .print_to_string()
                    .to_string();
                backend
                    .module
                    .verify()
                    .unwrap_or_else(|error| panic!("{kind}, {return_type:?}: {error}"));
                let owned = return_type.as_ref().is_some_and(|ty| !ty.is_unboxed());
                assert_eq!(
                    ir.matches("call void @molt_dec_ref_obj(i64 %direct_call)")
                        .count(),
                    usize::from(owned && !bound),
                    "{kind}, {return_type:?}, bound={bound}: {ir}",
                );
                if !bound {
                    assert!(
                        !ir.contains("@molt_int_from_i64("),
                        "discarded scalar must not allocate: {ir}"
                    );
                }
            }
        }
    }
}

#[test]
fn hash_constructors_share_typed_and_preserved_failure_cfg() {
    for (opcode, preserved, new_symbol, mutate_symbol, width) in [
        (OpCode::BuildDict, None, "molt_dict_new", "molt_dict_set", 2),
        (OpCode::BuildSet, None, "molt_set_new", "molt_set_add", 1),
        (
            OpCode::Copy,
            Some("dict_new"),
            "molt_dict_new",
            "molt_dict_set",
            2,
        ),
        (
            OpCode::Copy,
            Some("set_new"),
            "molt_set_new",
            "molt_set_add",
            1,
        ),
        (
            OpCode::Copy,
            Some("frozenset_new"),
            "molt_frozenset_new",
            "molt_frozenset_add",
            1,
        ),
    ] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "hash_literal_transaction".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let raw = func.fresh_value();
        let result = func.fresh_value();
        let mut attrs = AttrDict::new();
        if let Some(kind) = preserved {
            attrs.insert("_original_kind".into(), AttrValue::Str(kind.into()));
        }
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int_def(raw, i64::MAX));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![raw; width * 2],
            results: vec![result],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        backend.module.verify().expect("owned hash aggregate CFG");
        let ir = llvm_fn.print_to_string().to_string();
        assert!(ir.contains(&format!("@{new_symbol}(i64 2)")), "{ir}");
        assert!(ir.contains(&format!("call i64 @{mutate_symbol}")), "{ir}");
        assert!(
            ir.find("aggregate_created").unwrap() < ir.find("call i64 @molt_int_from_i64").unwrap(),
            "allocation failure must branch before any operand boxing/hashing: {ir}"
        );
        assert!(
            ir.contains("aggregate_abort")
                && ir.matches("call void @molt_dec_ref_obj").count()
                    >= if width == 2 { 5 } else { 3 },
            "{ir}"
        );
        assert!(ir.contains("aggregate_result = phi i64"), "{ir}");
        assert!(
            !ir.contains("aggregate_result = phi i64 [ %aggregate_insert"),
            "mutator return must never replace the constructor owner: {ir}"
        );
        let mutations: Vec<_> = ir
            .match_indices(&format!("call i64 @{mutate_symbol}"))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(mutations.len(), 2, "{ir}");
        let mutation = mutations[0];
        assert!(
            ir[..mutation].contains("@molt_exception_pending()"),
            "boxing failure must precede mutation: {ir}"
        );
        assert!(
            ir[mutation..].contains("@molt_exception_pending()"),
            "mutation failure must precede commit: {ir}"
        );
        assert!(
            ir[mutations[0]..mutations[1]].contains("@molt_exception_pending()"),
            "first insertion failure must branch before later operand hashing/mutation: {ir}"
        );
        assert!(
            !ir.contains("_builder_"),
            "hash constructors have no scratch builder ABI: {ir}"
        );
        assert_eq!(
            ir.contains("call void @molt_dec_ref_obj(i64 %aggregate_insert"),
            width == 1,
            "generated mutator ownership must dispose owned set/frozenset returns without releasing borrowed dict returns: {ir}"
        );
    }
}

#[test]
fn handwritten_container_owned_results_release_when_discarded() {
    for (opcode, preserved, operand_count, result_name) in [
        (OpCode::BuildList, None, 1, "sequence_builder_result"),
        (OpCode::BuildTuple, None, 1, "sequence_builder_result"),
        (OpCode::BuildSet, None, 1, "aggregate_result"),
        (OpCode::BuildDict, None, 2, "aggregate_result"),
        (OpCode::Copy, Some("frozenset_new"), 1, "aggregate_result"),
        (OpCode::BuildSlice, None, 3, "slice"),
        (OpCode::GetIter, None, 1, "molt_iter_checked"),
        (OpCode::IterNext, None, 1, "molt_iter_next"),
        (OpCode::ForIter, None, 1, "molt_iter_next"),
    ] {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        if let Some(symbol) = match opcode {
            OpCode::GetIter => Some("molt_iter_checked"),
            OpCode::IterNext | OpCode::ForIter => Some("molt_iter_next"),
            _ => None,
        } {
            backend.runtime_callable_symbols.insert(symbol.into());
        }
        let mut func = TirFunction::new(
            "discard_container_result".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let fallback = func.fresh_value();
        let operands: Vec<_> = (0..operand_count).map(|_| func.fresh_value()).collect();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_none_def(fallback));
        for &operand in &operands {
            entry.ops.push(const_none_def(operand));
        }
        let mut attrs = AttrDict::new();
        if let Some(kind) = preserved {
            attrs.insert("_original_kind".into(), AttrValue::Str(kind.into()));
        }
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results: vec![],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![fallback],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        backend
            .module
            .verify()
            .unwrap_or_else(|error| panic!("{opcode:?} {preserved:?}: {error}"));
        let ir = llvm_fn.print_to_string().to_string();
        assert!(
            ir.contains(&format!("call void @molt_dec_ref_obj(i64 %{result_name}")),
            "{opcode:?} {preserved:?} must retire its discarded owner: {ir}"
        );
    }

    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "discard_unboxed_iter_results".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let fallback = func.fresh_value();
    let iterator = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(fallback));
    entry.ops.push(const_none_def(iterator));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::IterNextUnboxed,
        operands: vec![iterator],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![fallback],
    };
    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    backend
        .module
        .verify()
        .expect("discarded unboxed iterator results");
    let ir = llvm_fn.print_to_string().to_string();
    for result in ["iter_next_unboxed_value_load", "iter_next_unboxed"] {
        assert!(
            ir.contains(&format!("call void @molt_dec_ref_obj(i64 %{result}")),
            "discarded unboxed iterator result {result} must be retired: {ir}"
        );
    }
}

#[test]
fn iterator_calls_box_and_retire_raw_inputs() {
    for (opcode, symbol) in [
        (OpCode::GetIter, "molt_iter_checked"),
        (OpCode::IterNext, "molt_iter_next"),
        (OpCode::ForIter, "molt_iter_next"),
    ] {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend.runtime_callable_symbols.insert(symbol.into());
        let mut func = TirFunction::new(
            "raw_iterator_input".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let raw = func.fresh_value();
        let fallback = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int_def(raw, i64::MAX));
        entry.ops.push(const_none_def(fallback));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![raw],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![fallback],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        backend
            .module
            .verify()
            .unwrap_or_else(|error| panic!("{opcode:?}: {error}"));
        let ir = llvm_fn.print_to_string().to_string();
        let call = format!("call i64 @{symbol}(i64 %boxed_int)");
        let input_release = "call void @molt_dec_ref_obj(i64 %boxed_int)";
        assert!(
            ir.contains("call i64 @molt_int_from_i64(i64 9223372036854775807)"),
            "{ir}"
        );
        assert!(
            ir.contains(&call),
            "raw iterator input must reach {symbol} boxed: {ir}"
        );
        assert!(
            ir.contains(input_release),
            "temporary iterator owner must be retired: {ir}"
        );
        assert!(
            ir.find(&call).unwrap() < ir.find(input_release).unwrap(),
            "iterator input must remain owned through the runtime borrow: {ir}"
        );
        assert!(
            ir.contains(&format!("call void @molt_dec_ref_obj(i64 %{symbol})")),
            "discarded iterator result must be retired: {ir}"
        );
    }

    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "raw_unboxed_iterator_input".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let raw = func.fresh_value();
    let fallback = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_int_def(raw, i64::MAX));
    entry.ops.push(const_none_def(fallback));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::IterNextUnboxed,
        operands: vec![raw],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![fallback],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    backend
        .module
        .verify()
        .expect("raw unboxed iterator input ownership");
    let ir = llvm_fn.print_to_string().to_string();
    let call = "call i64 @molt_iter_next_unboxed(i64 %boxed_int, i64 %iter_next_unboxed_value_ptr)";
    let input_release = "call void @molt_dec_ref_obj(i64 %boxed_int)";
    assert!(
        ir.contains(call),
        "mixed iterator input must be boxed: {ir}"
    );
    assert!(
        ir.contains(input_release),
        "mixed iterator input owner must be retired: {ir}"
    );
    assert!(
        ir.find(call).unwrap() < ir.find(input_release).unwrap(),
        "{ir}"
    );
    for result in ["iter_next_unboxed_value_load", "iter_next_unboxed"] {
        assert!(
            ir.contains(&format!("call void @molt_dec_ref_obj(i64 %{result})")),
            "discarded mixed iterator result {result} must be retired: {ir}"
        );
    }
}

#[test]
fn lower_call_guarded_uses_runtime_callable_dispatch_even_with_known_target() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    let _target = backend.module.add_function(
        "guarded_target",
        ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
        Some(inkwell::module::Linkage::External),
    );
    backend.function_linkage_abis.insert(
        "guarded_target".to_string(),
        test_native_linkage_abi(vec![TirType::DynBox], Some(TirType::DynBox)),
    );

    let mut func = TirFunction::new(
        "guarded_call_abi".into(),
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
    let mut func = TirFunction::new(
        "import_var_fallback".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
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
    let mut func = TirFunction::new(
        "container_builder_boxing".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
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
    assert!(!ir.contains("molt_set_add(i64 %aggregate, i64 2)"), "{ir}");
    assert!(
        !ir.contains("molt_dict_set(i64 %aggregate, i64 %str_bits, i64 2)"),
        "{ir}"
    );
}

#[test]
fn lower_preserved_container_builders_use_declared_append_abis() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "preserved_container_builder_append_abi".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
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
    assert!(ir.contains("call i32 @molt_list_builder_append"), "{ir}");
    assert!(ir.contains("sequence_item_admitted"), "{ir}");
    assert!(ir.contains("sequence_builder_abort"), "{ir}");
    assert!(ir.contains("sequence_builder_result = phi i64"), "{ir}");
    assert!(ir.contains("call i64 @molt_dict_set"), "{ir}");
    assert!(ir.contains("call i64 @molt_set_add"), "{ir}");
    assert!(
        ir.contains("aggregate_abort") && ir.contains("call void @molt_dec_ref_obj"),
        "{ir}"
    );
    assert!(ir.contains("@molt_exception_pending()"), "{ir}");
}

#[test]
fn list_and_tuple_builders_share_owned_failure_cfg_for_typed_and_preserved_ops() {
    for (opcode, preserved, finish) in [
        (OpCode::BuildList, None, "molt_list_builder_finish"),
        (OpCode::BuildTuple, None, "molt_tuple_builder_finish"),
        (OpCode::Copy, Some("list_new"), "molt_list_builder_finish"),
        (OpCode::Copy, Some("tuple_new"), "molt_tuple_builder_finish"),
    ] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "sequence_ownership".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let raw = func.fresh_value();
        let text = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int_def(raw, i64::MAX));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstStr,
            operands: vec![],
            results: vec![text],
            attrs: AttrDict::from([("s_value".into(), AttrValue::Str("borrowed".into()))]),
            source_span: None,
        });
        let mut attrs = AttrDict::new();
        if let Some(kind) = preserved {
            attrs.insert("_original_kind".into(), AttrValue::Str(kind.into()));
        }
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![raw, text],
            results: vec![result],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        backend
            .module
            .verify()
            .expect("sequence builder CFG must verify");
        let ir = llvm_fn.print_to_string().to_string();
        assert_eq!(
            ir.matches("call i32 @molt_list_builder_append").count(),
            2,
            "{ir}"
        );
        assert_eq!(
            ir.matches("call void @molt_dec_ref_obj").count(),
            2,
            "only the fresh raw-I64 box and partial builder are released, never the borrowed text: {ir}"
        );
        assert!(ir.contains("sequence_builder_created"), "{ir}");
        assert!(ir.contains("sequence_item_admitted"), "{ir}");
        assert!(ir.contains("sequence_builder_result = phi i64"), "{ir}");
        assert_eq!(
            ir.matches(&format!("call i64 @{finish}")).count(),
            1,
            "{ir}"
        );
        assert_eq!(
            ir.matches("ret i64").count(),
            1,
            "abort must not introduce a private return: {ir}"
        );
    }
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
        molt_ir::FunctionReturnAbi::Value,
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
    let mut func = TirFunction::new(
        "call_method_ic_attr_reuse".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
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
        molt_ir::FunctionReturnAbi::Value,
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
    let mut func = TirFunction::new(
        "class_def_boxed_attrs".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
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
