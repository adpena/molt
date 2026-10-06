use super::*;

fn runtime_call_shape_function(
    opcode: OpCode,
    kind: &str,
    symbol: &str,
    arity: usize,
    with_result: bool,
    raw_argument: bool,
) -> TirFunction {
    let mut func = TirFunction::new(
        format!("runtime_shape_{kind}_{opcode:?}_{with_result}"),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let fallback = func.fresh_value();
    let argument = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(fallback));
    entry.ops.push(if raw_argument {
        const_int_def(argument, i64::MAX)
    } else {
        const_none_def(argument)
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![argument; arity],
        results: if with_result { vec![result] } else { vec![] },
        attrs: AttrDict::from([
            (
                "_original_kind".into(),
                AttrValue::Str(if opcode == OpCode::Call { "call" } else { kind }.into()),
            ),
            ("s_value".into(), AttrValue::Str(symbol.into())),
        ]),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![if with_result { result } else { fallback }],
    };
    func
}

#[test]
fn named_builtin_llvm_lowering_never_drops_the_first_argument() {
    for (named, operand_count, expected_arguments) in
        [(true, 0, 0), (true, 1, 1), (true, 2, 2), (false, 2, 1)]
    {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            "builtin_arguments".into(),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let operands: Vec<_> = (0..operand_count).map(|_| func.fresh_value()).collect();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        for &operand in &operands {
            entry.ops.push(const_none_def(operand));
        }
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::CallBuiltin,
            operands,
            results: vec![result],
            attrs: if named {
                AttrDict::from([("name".into(), AttrValue::Str("len".into()))])
            } else {
                AttrDict::new()
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let ir = try_lower_tir_to_llvm(&func, &backend)
            .expect("valid builtin call contract")
            .print_to_string()
            .to_string();
        assert_eq!(
            ir.matches("@molt_callargs_push_pos(").count(),
            expected_arguments,
            "{ir}"
        );
        assert!(ir.contains("@molt_call_builtin("), "{ir}");
        assert_eq!(
            ir.matches("@molt_dec_ref_obj(").count(),
            usize::from(named),
            "only synthesized names are temporary owners; dynamic names remain borrowed: {ir}"
        );
    }
}

/// The preserved-op passthrough-class closure: each kind that previously
/// fell to the `Copy` operand-0 passthrough (a silent miscompile / dropped
/// side effect) must now lower to its dedicated runtime call. This pins the
/// specific dedicated arms whose runtime symbol DIFFERS from `molt_<kind>`
/// (so the generic fallback would have declined) or which are result-less.
#[test]
fn lower_preserved_passthrough_class_routes_to_runtime() {
    // (kind, n_operands, with_result, s_value, expected runtime symbol)
    let cases: &[(&str, usize, bool, Option<&str>, &str)] = &[
        ("abs", 1, true, None, "molt_abs_builtin"),
        ("const_ellipsis", 0, true, None, "molt_ellipsis"),
        (
            "const_not_implemented",
            0,
            true,
            None,
            "molt_not_implemented",
        ),
        ("gen_throw", 2, true, None, "molt_generator_throw"),
        ("gen_close", 1, true, None, "molt_generator_close"),
        (
            "get_attr_special_obj",
            1,
            true,
            Some("__class__"),
            "molt_get_attr_special",
        ),
        ("borrow", 1, true, None, "molt_inc_ref_obj"),
        ("binding_alias", 1, true, None, "molt_inc_ref_obj"),
        ("release", 1, true, None, "molt_dec_ref_obj"),
        ("guard_tag", 2, false, None, "molt_guard_type"),
        ("guard_tag", 2, true, None, "molt_guard_type"),
        ("guard_type", 2, false, None, "molt_guard_type"),
        ("guard_type", 2, true, None, "molt_guard_type"),
        ("guard_layout", 3, true, None, "molt_guard_layout"),
        ("guard_dict_shape", 3, true, None, "molt_guard_layout"),
        ("dataclass_new", 4, true, None, "molt_dataclass_new"),
        ("json_parse", 1, true, None, "molt_json_parse_scalar_obj"),
        (
            "msgpack_parse",
            1,
            true,
            None,
            "molt_msgpack_parse_scalar_obj",
        ),
        ("cbor_parse", 1, true, None, "molt_cbor_parse_scalar_obj"),
        (
            "stateful_locals_register",
            2,
            false,
            Some("gen_fn"),
            "molt_stateful_locals_register",
        ),
        (
            "function_closure_bits",
            1,
            true,
            None,
            "molt_function_closure_bits",
        ),
        ("asyncgen_new", 1, true, None, "molt_asyncgen_new"),
    ];
    for &(kind, nops, with_result, s_value, sym) in cases {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend.function_linkage_abis.insert(
            "gen_fn".to_string(),
            test_native_linkage_abi(vec![], Some(TirType::DynBox)),
        );
        let ir = lower_preserved_kind_ir(&backend, kind, nops, with_result, s_value)
            .unwrap_or_else(|e| {
                panic!(
                    "preserved `{kind}` must lower, got error: {:?}",
                    e.diagnostics()
                )
            });
        assert!(
            ir.contains(sym),
            "preserved `{kind}` must lower to `{sym}` (not an operand-0 \
                 passthrough); IR:\n{ir}"
        );
    }
}

fn builtin_callable_function(symbol: &str, arity: i64, named: bool) -> TirFunction {
    let mut function = runtime_call_shape_function(
        OpCode::Copy,
        "builtin_func",
        symbol,
        usize::from(named),
        true,
        false,
    );
    let operation = function
        .blocks
        .get_mut(&function.entry_block)
        .unwrap()
        .ops
        .last_mut()
        .unwrap();
    operation
        .attrs
        .insert("value".into(), AttrValue::Int(arity));
    function
}

#[test]
fn runtime_builtin_callable_uses_manifest_without_compiled_linkage() {
    // Independent signatures of runtime exports, including the five-word
    // __import__ entry that failed in a real shared-stdlib batch.
    for (symbol, arity) in [
        ("molt_sys_version", 0),
        ("molt_abs_builtin", 1),
        ("molt_socket_drop", 1),
        ("molt_importlib_import_transaction", 5),
    ] {
        for named in [false, true] {
            let ctx = Context::create();
            let backend = make_backend(&ctx);
            assert!(backend.function_linkage_abis.is_empty());
            let function = builtin_callable_function(symbol, arity, named);
            try_lower_tir_to_llvm(&function, &backend).unwrap();
            backend.module.verify().unwrap();
            let target = backend.module.get_function(symbol).unwrap();
            assert_eq!(target.count_params(), arity as u32);
            assert_eq!(
                target.get_type().get_return_type(),
                Some(ctx.i64_type().into())
            );
            let trampoline = backend
                .module
                .get_function(&format!("{symbol}__molt_llvm_trampoline_{arity}"))
                .unwrap()
                .print_to_string()
                .to_string();
            assert_eq!(
                trampoline.matches("load i64,").count(),
                arity as usize,
                "{trampoline}"
            );
            assert!(
                trampoline.contains(&format!("call i64 @{symbol}(")),
                "{trampoline}"
            );
            assert!(
                !trampoline.contains("molt_int_as_i64"),
                "boxed integer arguments must retain all bits: {trampoline}"
            );
            assert!(
                !trampoline.contains("molt_dec_ref"),
                "runtime entries borrow the boxed arguments: {trampoline}"
            );
        }
    }
}

#[test]
fn runtime_builtin_callable_rejects_unknown_raw_and_wrong_arity() {
    for (symbol, arity, diagnostic) in [
        ("user_function", 0, "has no runtime callable ABI"),
        ("molt_int_from_i64", 1, "has no runtime callable ABI"),
        (
            "molt_dict_getitem_borrowed",
            2,
            "has no runtime callable ABI",
        ),
        ("molt_importlib_import_transaction", 4, "arity mismatch"),
    ] {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        // Neither machine signature nor a compiled linkage row authorizes
        // publication as a runtime builtin.
        backend.function_linkage_abis.insert(
            symbol.into(),
            test_native_linkage_abi(vec![TirType::DynBox; arity as usize], Some(TirType::DynBox)),
        );
        let function = builtin_callable_function(symbol, arity, false);
        let error = try_lower_tir_to_llvm(&function, &backend).unwrap_err();
        assert_lowering_error_contains(&error, diagnostic);
        assert_lowering_error_contains(&error, symbol);
    }
}

#[test]
fn runtime_builtin_call_frame_uses_provider_as_trampoline() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let symbol = "molt_cpython_abi_cext_call_trampoline";
    let function = builtin_callable_function(symbol, 3, false);
    let lowered = try_lower_tir_to_llvm(&function, &backend).unwrap();
    backend.module.verify().unwrap();
    let ir = lowered.print_to_string().to_string();
    assert_eq!(ir.matches(&format!("ptr @{symbol}")).count(), 2, "{ir}");
    assert!(
        backend
            .module
            .get_function(&format!("{symbol}__molt_llvm_trampoline_3"))
            .is_none(),
        "call frame must not be unpacked as Python positional arguments"
    );
}

#[test]
fn callable_constructors_release_only_discarded_owned_results() {
    for (kind, argc) in [
        ("func_new", 0),
        ("func_new_closure", 1),
        ("builtin_func", 0),
        ("code_new", 9),
        ("callargs_new", 0),
        ("classmethod_new", 1),
        ("staticmethod_new", 1),
        ("property_new", 3),
        ("bound_method_new", 2),
        ("asyncgen_new", 1),
    ] {
        for bound in [false, true] {
            let ctx = Context::create();
            let mut backend = make_backend(&ctx);
            // Generic boxed calls require both semantic ABI classification
            // and availability in the selected linked runtime profile.
            backend
                .runtime_callable_symbols
                .insert(format!("molt_{kind}"));
            let mut target_abi = test_native_linkage_abi(
                if kind == "func_new_closure" {
                    vec![TirType::DynBox]
                } else {
                    vec![]
                },
                Some(TirType::DynBox),
            );
            // A closure constructor's target takes the closure transport first.
            target_abi.source_signature.has_closure = kind == "func_new_closure";
            if kind != "builtin_func" {
                backend
                    .function_linkage_abis
                    .insert("callable_result_target".into(), target_abi);
            }
            let symbol_target = match kind {
                "builtin_func" => Some("molt_sys_version"),
                "func_new" | "func_new_closure" => Some("callable_result_target"),
                _ => None,
            };
            let ir = lower_preserved_kind_ir(&backend, kind, argc, bound, symbol_target)
                .unwrap_or_else(|error| panic!("{kind}: {:?}", error.diagnostics()));
            backend
                .module
                .verify()
                .unwrap_or_else(|error| panic!("{kind}, bound={bound}: {error}"));
            let symbol = if kind == "builtin_func" {
                "molt_func_new_builtin".into()
            } else {
                format!("molt_{kind}")
            };
            assert!(ir.contains(&format!("call i64 @{symbol}(")), "{ir}");
            assert_eq!(
                ir.matches("call void @molt_dec_ref_obj(").count(),
                usize::from(!bound),
                "{kind}, bound={bound}: {ir}"
            );
            assert!(
                !ir.contains("call void @molt_inc_ref_obj("),
                "owned constructor result is transferred, not retained: {ir}"
            );
        }
    }
}

#[test]
fn descriptor_constructors_share_boxed_admission_and_argument_materialization() {
    for (kind, arity) in [
        ("classmethod_new", 1),
        ("staticmethod_new", 1),
        ("property_new", 3),
        ("bound_method_new", 2),
    ] {
        let symbol = format!("molt_{kind}");
        for (available, supplied, diagnostic) in [
            (false, arity, "is unavailable in the selected runtime"),
            (
                true,
                arity - 1,
                "no positional boxed-value ABI classification",
            ),
            (
                true,
                arity + 1,
                "no positional boxed-value ABI classification",
            ),
        ] {
            // A lowering failure leaves the attempted function body in its
            // module. Each independent admission case owns a fresh backend;
            // it must not test accidental function redefinition instead.
            let ctx = Context::create();
            let mut backend = make_backend(&ctx);
            if available {
                backend.runtime_callable_symbols.insert(symbol.clone());
            } else {
                backend.runtime_callable_symbols.remove(&symbol);
            }
            let error = lower_preserved_kind_ir(&backend, kind, supplied, true, None)
                .expect_err("descriptor construction must enforce generated ABI admission");
            assert_lowering_error_contains(&error, diagnostic);
            assert!(backend.module.get_function(&symbol).is_none());
        }

        // A raw integer carrier is not already a Python object. The common
        // runtime route must materialize each argument according to its type.
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend.runtime_callable_symbols.insert(symbol.clone());
        let mut func = TirFunction::new(
            format!("boxed_{kind}"),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let input = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int_def(input, 7));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![input; arity],
            results: vec![result],
            attrs: AttrDict::from([("_original_kind".into(), AttrValue::Str(kind.into()))]),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let ir = try_lower_tir_to_llvm(&func, &backend)
            .unwrap_or_else(|error| panic!("{kind}: {:?}", error.diagnostics()))
            .print_to_string()
            .to_string();
        let boxed = (nanbox::QNAN | nanbox::TAG_INT | 7) as i64;
        let call = ir
            .lines()
            .find(|line| line.contains(&format!("call i64 @{symbol}(")))
            .unwrap_or_else(|| panic!("missing descriptor call: {ir}"));
        assert_eq!(call.matches("i64 %boxed_int").count(), arity, "{ir}");
        assert_eq!(
            ir.matches("call void @molt_dec_ref_obj(i64 %boxed_call_owner_bits")
                .count(),
            1,
            "repeated descriptor operands share one materialized owner: {ir}"
        );
        assert_eq!(
            ir.matches(&format!("phi i64 [ {boxed}, %box_int_inline"))
                .count(),
            1,
            "repeated descriptor operands preserve identity: {ir}"
        );
        backend.module.verify().expect("boxed descriptor arguments");
    }
}

#[test]
fn dedicated_runtime_results_preserve_borrowed_and_owned_custody() {
    for kind in ["alloc_class", "asyncgen_new", "function_closure_bits"] {
        for keep_result in [false, true] {
            let ctx = Context::create();
            let backend = make_backend(&ctx);
            let mut func = TirFunction::new(
                format!("dedicated_{kind}_{keep_result}"),
                vec![TirType::DynBox],
                TirType::DynBox,
                molt_ir::FunctionReturnAbi::Value,
            );
            let result = func.fresh_value();
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            let operand = entry.args[0].id;
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![operand],
                results: if keep_result { vec![result] } else { vec![] },
                attrs: AttrDict::from([
                    ("_original_kind".into(), AttrValue::Str(kind.into())),
                    ("value".into(), AttrValue::Int(48)),
                ]),
                source_span: None,
            });
            entry.terminator = Terminator::Return {
                values: vec![if keep_result { result } else { operand }],
            };
            let ir = try_lower_tir_to_llvm(&func, &backend)
                .expect("dedicated runtime ABI must lower")
                .print_to_string()
                .to_string();
            backend
                .module
                .verify()
                .expect("dedicated runtime ABI must verify");
            let expected_call = if kind == "alloc_class" {
                "call i64 @molt_alloc_class(i64 48, i64 %0)".to_string()
            } else {
                format!("call i64 @molt_{kind}(i64 %0)")
            };
            assert!(ir.contains(&expected_call), "{ir}");
            assert!(!ir.contains("@molt_int_from_i64("), "{ir}");
            if kind == "alloc_class" {
                let publish = "call i64 @molt_object_publish_initialized(i64 %alloc_class)";
                assert!(ir.contains(publish), "{ir}");
                assert!(
                    ir.find(&expected_call).unwrap() < ir.find(publish).unwrap(),
                    "{ir}"
                );
                if !keep_result {
                    assert!(
                        ir.contains("call void @molt_dec_ref_obj(i64 %class_initialized)"),
                        "{ir}"
                    );
                }
            }
            if kind == "asyncgen_new" && !keep_result {
                assert!(
                    ir.contains("call void @molt_dec_ref_obj(i64 %asyncgen_new)"),
                    "{ir}"
                );
            }
            if keep_result {
                let result_name = if kind == "alloc_class" {
                    "class_initialized"
                } else {
                    kind
                };
                assert!(ir.contains(&format!("ret i64 %{result_name}")), "{ir}");
            }
            let borrowed = kind == "function_closure_bits";
            assert_eq!(
                ir.matches("call void @molt_inc_ref_obj(").count(),
                usize::from(borrowed && keep_result),
                "only binding a borrowed closure acquires an owner: {ir}"
            );
            assert_eq!(
                ir.matches("call void @molt_dec_ref_obj(").count(),
                usize::from(!borrowed && !keep_result),
                "only discarding an owned result retires an owner: {ir}"
            );
            if borrowed && keep_result {
                assert!(
                    ir.contains("call void @molt_inc_ref_obj(i64 %function_closure_bits)"),
                    "{ir}"
                );
            }
        }
    }
}

#[test]
fn retired_exact_runtime_kinds_take_the_admitted_boxed_route() {
    // Kinds whose runtime entry is exactly `molt_<kind>` with a generated row
    // have no dedicated arm: a raw integer operand is boxed once, owned through
    // the call and released after it, and an unavailable symbol fails closed.
    for (kind, arity) in [
        ("isinstance", 2),
        ("issubclass", 2),
        ("has_attr_name", 2),
        ("is_callable", 1),
        ("str_from_obj", 1),
        ("int_from_obj", 3),
        ("ord", 1),
        ("string_join", 2),
        ("module_set_attr", 3),
        ("exception_stack_exit", 1),
        ("context_unwind_to", 2),
        ("class_merge_layout", 3),
        ("callargs_push_pos", 2),
        ("callargs_expand_star", 2),
        ("code_new", 9),
        ("vec_sum", 3),
        ("vec_prod", 3),
        ("vec_min", 3),
        ("vec_max", 3),
    ] {
        let symbol = format!("molt_{kind}");
        for admitted in [false, true] {
            let ctx = Context::create();
            let mut backend = make_backend(&ctx);
            if admitted {
                backend.runtime_callable_symbols.insert(symbol.clone());
            }
            let func = runtime_call_shape_function(OpCode::Copy, kind, &symbol, arity, true, true);
            if !admitted {
                let error = try_lower_tir_to_llvm(&func, &backend)
                    .expect_err("an exact runtime kind requires runtime admission");
                assert_lowering_error_contains(
                    &error,
                    &format!(
                        "boxed runtime symbol `{symbol}` is unavailable in the selected runtime"
                    ),
                );
                continue;
            }
            let ir = try_lower_tir_to_llvm(&func, &backend)
                .unwrap_or_else(|error| panic!("{kind}: {:?}", error.diagnostics()))
                .print_to_string()
                .to_string();
            backend
                .module
                .verify()
                .unwrap_or_else(|error| panic!("{kind}: {error}"));
            let call = ir
                .lines()
                .find(|line| line.contains(&format!("call i64 @{symbol}(")))
                .unwrap_or_else(|| panic!("{kind}: missing runtime call: {ir}"));
            assert_eq!(
                call.matches("i64 %boxed_int").count(),
                arity,
                "{kind}: {ir}"
            );
            assert_eq!(
                ir.matches("call i64 @molt_int_from_i64(").count(),
                1,
                "{kind}: {ir}"
            );
            let release = "call void @molt_dec_ref_obj(i64 %boxed_call_owner_bits)";
            assert_eq!(ir.matches(release).count(), 1, "{kind}: {ir}");
            assert!(
                ir.find(call).unwrap() < ir.find(release).unwrap(),
                "{kind}: the box outlives the borrowing call: {ir}"
            );
        }
    }
}

#[test]
fn dedicated_object_abi_arms_box_raw_operands_and_keep_raw_words() {
    // (kind, operands, raw tag attr, runtime symbol, expected arguments)
    let cases: &[(&str, usize, Option<i64>, &str, &str)] = &[
        ("type_of", 1, None, "molt_type_of", "(i64 %boxed_int)"),
        (
            "builtin_type",
            1,
            None,
            "molt_builtin_type",
            "(i64 %boxed_int)",
        ),
        (
            "get_attr_name_default",
            3,
            None,
            "molt_get_attr_name_default",
            "(i64 %boxed_int, i64 %boxed_int, i64 %boxed_int)",
        ),
        (
            "gen_send",
            2,
            None,
            "molt_generator_send",
            "(i64 %boxed_int, i64 %boxed_int)",
        ),
        (
            "gen_throw",
            2,
            None,
            "molt_generator_throw",
            "(i64 %boxed_int, i64 %boxed_int)",
        ),
        (
            "gen_close",
            1,
            None,
            "molt_generator_close",
            "(i64 %boxed_int)",
        ),
        (
            "super_new",
            2,
            None,
            "molt_super_new",
            "(i64 %boxed_int, i64 %boxed_int)",
        ),
        (
            "class_layout_version",
            1,
            None,
            "molt_class_layout_version",
            "(i64 %boxed_int)",
        ),
        ("abs", 1, None, "molt_abs_builtin", "(i64 %boxed_int)"),
        (
            "string_format",
            2,
            None,
            "molt_format_builtin",
            "(i64 %boxed_int, i64 %boxed_int)",
        ),
        (
            "json_parse",
            1,
            None,
            "molt_json_parse_scalar_obj",
            "(i64 %boxed_int)",
        ),
        (
            "exception_match_builtin",
            1,
            Some(7),
            "molt_exception_match_builtin",
            "(i64 %boxed_int, i64 7)",
        ),
        (
            "exception_new_builtin_one",
            1,
            Some(7),
            "molt_exception_new_builtin_one",
            "(i64 7, i64 %boxed_int)",
        ),
        (
            "exception_new_builtin",
            1,
            Some(7),
            "molt_exception_new_builtin",
            "(i64 7, i64 %boxed_int)",
        ),
    ];
    for &(kind, arity, tag, symbol, arguments) in cases {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            format!("dedicated_{kind}"),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let raw = func.fresh_value();
        let result = func.fresh_value();
        let mut attrs = AttrDict::from([("_original_kind".into(), AttrValue::Str(kind.into()))]);
        if let Some(tag) = tag {
            attrs.insert("value".into(), AttrValue::Int(tag));
        }
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(const_int_def(raw, i64::MAX));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![raw; arity],
            results: vec![result],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let ir = try_lower_tir_to_llvm(&func, &backend)
            .unwrap_or_else(|error| panic!("{kind}: {:?}", error.diagnostics()))
            .print_to_string()
            .to_string();
        backend
            .module
            .verify()
            .unwrap_or_else(|error| panic!("{kind}: {error}"));
        let call = format!("call i64 @{symbol}{arguments}");
        let release = format!("call void @molt_dec_ref_obj(i64 %{kind}_owner_bits)");
        assert!(
            ir.contains(&call),
            "{kind}: object operands are boxed and raw ABI words stay raw: {ir}"
        );
        assert_eq!(
            ir.matches("call i64 @molt_int_from_i64(").count(),
            1,
            "{kind}: {ir}"
        );
        assert_eq!(ir.matches(&release).count(), 1, "{kind}: {ir}");
        assert!(
            ir.find(&call).unwrap() < ir.find(&release).unwrap(),
            "{kind}: the box outlives the borrowing call: {ir}"
        );
    }

    // A module store borrows its operands and retires its discarded result.
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "module_store_owner".into(),
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
        opcode: OpCode::ModuleSetAttr,
        operands: vec![raw; 3],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![fallback],
    };
    let ir = try_lower_tir_to_llvm(&func, &backend)
        .unwrap_or_else(|error| panic!("module store: {:?}", error.diagnostics()))
        .print_to_string()
        .to_string();
    backend.module.verify().expect("module store ownership");
    let call = "call i64 @molt_module_set_attr(i64 %boxed_int, i64 %boxed_int, i64 %boxed_int)";
    let release = "call void @molt_dec_ref_obj(i64 %module_call_owner_bits)";
    assert!(ir.contains(call), "{ir}");
    assert!(ir.find(call).unwrap() < ir.find(release).unwrap(), "{ir}");
    assert!(
        ir.contains("call void @molt_dec_ref_obj(i64 %module_call_result)"),
        "a discarded owned module result is released: {ir}"
    );
}

#[test]
fn direct_and_preserved_boxed_calls_share_result_custody() {
    for opcode in [OpCode::Call, OpCode::Copy] {
        for (kind, symbol, arity, shape) in [
            ("dict_set", "molt_dict_set", 3, "borrowed"),
            ("dict_update", "molt_dict_update", 2, "owned"),
            ("dict_update_kwstar", "molt_dict_update_kwstar", 2, "owned"),
            (
                "dict_update_missing",
                "molt_dict_update_missing",
                3,
                "borrowed",
            ),
            ("list_append", "molt_list_append", 2, "owned"),
            ("print_newline", "molt_print_newline", 0, "void"),
            ("vec_sum", "molt_vec_sum", 3, "owned"),
            ("vec_prod", "molt_vec_prod", 3, "owned"),
            ("vec_min", "molt_vec_min", 3, "owned"),
            ("vec_max", "molt_vec_max", 3, "owned"),
        ] {
            for with_result in [false, true] {
                let ctx = Context::create();
                let mut backend = make_backend(&ctx);
                backend.runtime_callable_symbols.insert(symbol.into());
                let func =
                    runtime_call_shape_function(opcode, kind, symbol, arity, with_result, false);

                if shape == "void" && with_result {
                    let error = try_lower_tir_to_llvm(&func, &backend)
                        .expect_err("void boxed calls cannot bind a result");
                    assert_lowering_error_contains(&error, "has result values");
                    continue;
                }

                let ir = try_lower_tir_to_llvm(&func, &backend)
                    .unwrap_or_else(|error| {
                        panic!(
                            "{opcode:?} {kind}, bound={with_result}: {:?}",
                            error.diagnostics()
                        )
                    })
                    .print_to_string()
                    .to_string();
                let call_abi = if shape == "void" { "void" } else { "i64" };
                assert!(ir.contains(&format!("call {call_abi} @{symbol}(")), "{ir}");
                assert_eq!(
                    ir.matches("call void @molt_inc_ref_obj(").count(),
                    usize::from(shape == "borrowed" && with_result),
                    "only a bound borrowed result acquires ownership: {ir}"
                );
                assert_eq!(
                    ir.matches("call void @molt_dec_ref_obj(").count(),
                    usize::from(shape == "owned" && !with_result),
                    "only an unbound owned result is disposed: {ir}"
                );
                backend
                    .module
                    .verify()
                    .unwrap_or_else(|error| panic!("{opcode:?} {kind}: {error}"));
            }
        }
    }
}

#[test]
fn preserved_vector_reductions_reject_wrong_arity_before_materialization() {
    for kind in ["vec_sum", "vec_prod", "vec_min", "vec_max"] {
        let symbol = format!("molt_{kind}");
        for arity in [2, 4] {
            let ctx = Context::create();
            let mut backend = make_backend(&ctx);
            backend.runtime_callable_symbols.insert(symbol.clone());
            let func = runtime_call_shape_function(OpCode::Copy, kind, &symbol, arity, true, true);
            let error = try_lower_tir_to_llvm(&func, &backend)
                .expect_err("a preserved reduction must carry exactly three object operands");
            assert_lowering_error_contains(
                &error,
                "has no positional boxed-value ABI classification",
            );
            assert!(backend.module.get_function(&symbol).is_none());
            let ir = backend.module.print_to_string().to_string();
            assert!(
                !ir.contains("call i64 @molt_int_from_i64("),
                "{kind}/{arity}: {ir}"
            );
            assert!(
                !ir.contains("call void @molt_dec_ref_obj("),
                "{kind}/{arity}: {ir}"
            );
        }
    }
}

#[test]
fn direct_and_preserved_borrowed_calls_box_raw_arguments_before_cleanup() {
    for opcode in [OpCode::Call, OpCode::Copy] {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend
            .runtime_callable_symbols
            .insert("molt_dict_set".into());
        let func = runtime_call_shape_function(opcode, "dict_set", "molt_dict_set", 3, true, true);
        let ir = try_lower_tir_to_llvm(&func, &backend)
            .unwrap_or_else(|error| panic!("{opcode:?}: {:?}", error.diagnostics()))
            .print_to_string()
            .to_string();
        let call = ir
            .lines()
            .find(|line| line.contains("call i64 @molt_dict_set("))
            .unwrap_or_else(|| panic!("missing borrowed dict call: {ir}"));
        assert_eq!(call.matches("i64 %boxed_int").count(), 3, "{ir}");
        assert_eq!(
            ir.matches("call i64 @molt_int_from_i64(").count(),
            1,
            "the same raw value in three positions must keep one object identity: {ir}"
        );
        assert_eq!(
            ir.matches("call void @molt_dec_ref_obj(i64 %boxed_call_owner_bits")
                .count(),
            1,
            "the single materialized owner must be retired exactly once: {ir}"
        );
        let retain = ir
            .find("call void @molt_inc_ref_obj(i64 %molt_dict_set)")
            .unwrap_or_else(|| panic!("bound borrowed result was not retained: {ir}"));
        let cleanup = ir
            .find("call void @molt_dec_ref_obj(i64 %boxed_call_owner_bits")
            .unwrap_or_else(|| panic!("temporary argument owner was not retired: {ir}"));
        assert!(
            retain < cleanup,
            "borrowed result must be retained before cleanup: {ir}"
        );
        backend
            .module
            .verify()
            .expect("borrowed boxed raw arguments");
    }
}

#[test]
fn direct_and_preserved_boxed_calls_require_runtime_symbol_admission() {
    for opcode in [OpCode::Call, OpCode::Copy] {
        for (kind, symbol, arity) in [
            ("dict_set", "molt_dict_set", 3),
            ("dict_update", "molt_dict_update", 2),
            ("dict_update_kwstar", "molt_dict_update_kwstar", 2),
            ("dict_update_missing", "molt_dict_update_missing", 3),
        ] {
            for with_result in [false, true] {
                let ctx = Context::create();
                let mut backend = make_backend(&ctx);
                backend.runtime_callable_symbols.remove(symbol);
                let func =
                    runtime_call_shape_function(opcode, kind, symbol, arity, with_result, true);
                let error = try_lower_tir_to_llvm(&func, &backend)
                    .expect_err("generated semantics must not imply runtime availability");
                assert_lowering_error_contains(
                    &error,
                    &format!(
                        "boxed runtime symbol `{symbol}` is unavailable in the selected runtime"
                    ),
                );
                assert!(backend.module.get_function(symbol).is_none());
                let ir = backend.module.print_to_string().to_string();
                assert!(!ir.contains("call i64 @molt_int_from_i64("), "{ir}");
                assert!(!ir.contains("call void @molt_inc_ref_obj("), "{ir}");
                assert!(!ir.contains("call void @molt_dec_ref_obj("), "{ir}");
            }
        }
    }
}

#[test]
fn stateful_locals_registration_preserves_mixed_abi() {
    let kind = "stateful_locals_register";
    // Real IR registers a task poll entry with no arity claim; the linkage row
    // alone names its one task parameter. A present claim is checked instead.
    for claimed_arity in [None, Some(1)] {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend.function_linkage_abis.insert(
            "poll_fn".into(),
            test_native_linkage_abi(vec![TirType::DynBox], Some(TirType::DynBox)),
        );
        let mut func = TirFunction::new(
            format!("register_{kind}"),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let names = entry.args[0].id;
        let offsets = entry.args[1].id;
        let mut attrs = AttrDict::from([
            ("_original_kind".into(), AttrValue::Str(kind.into())),
            ("s_value".into(), AttrValue::Str("poll_fn".into())),
        ]);
        if let Some(arity) = claimed_arity {
            attrs.insert("value".into(), AttrValue::Int(arity));
        }
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![names, offsets],
            results: vec![result],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let ir = try_lower_tir_to_llvm(&func, &backend)
            .expect("generator locals mixed ABI must lower")
            .print_to_string()
            .to_string();
        backend
            .module
            .verify()
            .expect("generator locals mixed ABI must verify");
        assert!(
            ir.contains(&format!(
                "call i64 @molt_{kind}(i64 ptrtoint (ptr @poll_fn to i64), i64 %0, i64 %1)"
            )),
            "{ir}"
        );
        let poll_fn = backend.module.get_function("poll_fn").unwrap();
        assert_eq!(poll_fn.count_params(), 1, "{ir}");
        assert!(!ir.contains("@molt_int_from_i64("), "{ir}");
        assert!(!ir.contains("call void @molt_dec_ref_obj("), "{ir}");
    }
}

#[test]
fn layout_guards_preserve_tagged_parameters_at_runtime_admission() {
    for kind in ["guard_layout", "guard_dict_shape"] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            kind.into(),
            vec![
                TirType::UserClass("C".into()),
                TirType::DynBox,
                TirType::DynBox,
            ],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: entry.args.iter().map(|arg| arg.id).collect(),
            results: vec![result],
            attrs: AttrDict::from([("_original_kind".into(), AttrValue::Str(kind.into()))]),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let ir = try_lower_tir_to_llvm(&func, &backend)
            .expect("layout guard must lower")
            .print_to_string()
            .to_string();
        backend
            .module
            .verify()
            .expect("tagged layout guard ABI must verify");
        assert!(
            ir.contains("@molt_guard_layout(i64 %0, i64 %1, i64 %2)"),
            "layout guards must preserve the receiver tag: {ir}"
        );
        assert!(
            !ir.contains("inttoptr") && !ir.contains("ptr_unbox"),
            "{ir}"
        );
    }
}

#[test]
fn marked_finally_observer_lowers_to_the_fused_runtime_projection() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "marked_finally_observer".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    let mut observer = TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![],
        results: vec![result],
        attrs: AttrDict::from([(
            "_original_kind".into(),
            AttrValue::Str("exception_finally_pending_observer".into()),
        )]),
        source_span: None,
    };
    observer.mark_async_work_poll();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(observer);
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let ir = try_lower_tir_to_llvm(&func, &backend)
        .expect("marked observer must lower")
        .print_to_string()
        .to_string();
    assert!(
        ir.contains("molt_async_work_poll_and_exception_last_pending"),
        "marked observer must use the fused poll/object-return primitive:\n{ir}"
    );
    assert!(
        !ir.contains("call i64 @molt_exception_last_pending"),
        "marked observer must not emit the unfused runtime call:\n{ir}"
    );
}

#[test]
fn lower_special_get_attr_trusts_runtime_owned_result() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let ir = lower_preserved_kind_ir(&backend, "get_attr_special_obj", 1, true, Some("__class__"))
        .expect("special getattr must lower");
    assert!(ir.contains("molt_get_attr_special"), "{ir}");
    assert!(!ir.contains("get_attr_special_inc_ref"), "{ir}");
    assert!(!ir.contains("call void @molt_inc_ref_obj"), "{ir}");
}

/// Repr-identity preserved ops (`cast`, `widen`, `store_var`, `copy_var`, and
/// `identity_alias`) are the
/// explicit exception to the terminal preserved-op fail-loud rule: they
/// carry no runtime semantics and must alias operand 0 exactly, matching
/// native/WASM identity lowering over the NaN-boxed value format.
#[test]
fn lower_preserved_repr_identity_ops_pass_operand_through() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    for kind in ["cast", "widen", "store_var", "copy_var", "identity_alias"] {
        let mut func = TirFunction::new(
            format!("preserved_{kind}_identity"),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let src = func
            .blocks
            .get(&func.entry_block)
            .and_then(|block| block.args.first())
            .map(|arg| arg.id)
            .expect("identity test function must have one entry argument");
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let mut attrs = AttrDict::new();
        attrs.insert("_original_kind".into(), AttrValue::Str(kind.to_string()));
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![src],
            results: vec![result],
            attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let ir = try_lower_tir_to_llvm(&func, &backend)
            .map(|f| f.print_to_string().to_string())
            .unwrap_or_else(|e| {
                panic!(
                    "repr-identity preserved `{kind}` must lower as operand-0 \
                         passthrough, got error: {:?}",
                    e.diagnostics()
                )
            });
        assert!(
            !ir.contains("call "),
            "repr-identity preserved `{kind}` must not lower through a runtime call:\n{ir}"
        );
        assert!(
            ir.contains("ret i64 %0"),
            "repr-identity preserved `{kind}` must return operand 0 exactly:\n{ir}"
        );
    }
}

/// Terminal fail-loud state: a preserved `Copy` carrying an `_original_kind`
/// that NO arm and NO `molt_<kind>` runtime intrinsic claims must be a hard
/// `record_fatal` lowering error — never a silent operand-0 passthrough.
/// `__ppaudit_unmapped__` is a synthetic kind that cannot resolve to any
/// `molt_*` symbol, so it must reach the terminal guard.
#[test]
fn lower_preserved_unmapped_kind_fails_loud() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let err = lower_preserved_kind_ir(&backend, "__ppaudit_unmapped__", 1, true, None).expect_err(
        "an unhandled preserved op must fail the lowering, not silently \
                 pass operand 0 through",
    );
    assert_lowering_error_contains(&err, "unhandled preserved SimpleIR op");
    assert_lowering_error_contains(&err, "__ppaudit_unmapped__");
}

/// RESULT-LESS preserved side-effect ops (`print_newline`, `set_update`, …)
/// whose `molt_<kind>` symbol IS in the linked
/// intrinsic surface must lower to that runtime call via the generic
/// fallback — NOT be dropped as a `Copy` "0 results → no-op". The
/// passthrough enumeration found these reaching the no-op branch (a missing
/// newline / a set mutation that never happened). This pins the
/// result-less generic-fallback path; the symbols are injected because the
/// unit-test backend has an empty intrinsic surface by default.
#[test]
fn lower_preserved_resultless_side_effect_routes_to_runtime() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    // (kind, n_operands, expected runtime symbol). All result-less (res=0).
    let cases: &[(&str, usize, &str)] = &[
        ("print_newline", 0, "molt_print_newline"),
        ("set_update", 2, "molt_set_update"),
        ("math_sin", 1, "molt_math_sin"),
        (
            "string_split_field_len_from_bounds",
            4,
            "molt_string_split_field_len_from_bounds",
        ),
    ];
    for &(_, _, sym) in cases {
        backend.runtime_callable_symbols.insert(sym.to_string());
    }
    for &(kind, nops, sym) in cases {
        let ir = lower_preserved_kind_ir(&backend, kind, nops, false, None).unwrap_or_else(|e| {
            panic!(
                "result-less preserved `{kind}` must lower, got error: {:?}",
                e.diagnostics()
            )
        });
        assert!(
            ir.contains(sym),
            "result-less preserved `{kind}` must lower to `{sym}` (not a \
                 dropped no-op); IR:\n{ir}"
        );
        if sym == "molt_print_newline" {
            assert!(
                ir.contains("call void @molt_print_newline()"),
                "print_newline must use the runtime's void ABI; IR:\n{ir}"
            );
        }
    }
}

#[test]
fn lower_preserved_void_runtime_result_shape_fails_loud() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_print_newline".to_string());
    let err = lower_preserved_kind_ir(&backend, "print_newline", 0, true, None)
        .expect_err("void preserved runtime ops must not bind a boxed result");
    assert_lowering_error_contains(&err, "call to void runtime symbol");
    assert_lowering_error_contains(&err, "print_newline");
}

#[test]
fn preserved_runtime_calls_reject_raw_carriers_even_when_linked() {
    for (kind, arity) in [("int_from_i64", 1), ("int_as_i64", 1), ("is_truthy", 1)] {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend
            .runtime_callable_symbols
            .insert(format!("molt_{kind}"));
        let error = lower_preserved_kind_ir(&backend, kind, arity, true, None)
            .expect_err("machine i64 ABI cannot authorize boxed operands/results");
        assert_lowering_error_contains(&error, "no positional boxed-value ABI classification");
    }
}

/// The dual safety check: a result-less preserved op whose `molt_<kind>`
/// symbol is ABSENT from the intrinsic surface must STILL fail loud (never a
/// silent dropped side effect). Without the symbol the generic fallback
/// declines and the terminal guard must fire.
#[test]
fn lower_preserved_resultless_unmapped_fails_loud() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let err = lower_preserved_kind_ir(&backend, "__ppaudit_resultless__", 2, false, None)
        .expect_err("an unhandled result-less preserved op must fail the lowering");
    assert_lowering_error_contains(&err, "unhandled preserved SimpleIR op");
    assert_lowering_error_contains(&err, "__ppaudit_resultless__");
}

/// A bare `Copy` (no `_original_kind` — a genuine SSA value copy such as
/// `copy`/`load_var`/`store_var`) must STILL take the benign operand-0
/// passthrough. The terminal fail-loud guard keys on `_original_kind`, so it
/// must not fire here.
#[test]
fn lower_bare_copy_without_original_kind_passes_through() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "bare_copy".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let src = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(src));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![src],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    // Must lower cleanly (no fatal); the result aliases the source.
    let ir = try_lower_tir_to_llvm(&func, &backend)
        .map(|f| f.print_to_string().to_string())
        .expect("a bare Copy without _original_kind must lower as a passthrough");
    assert!(
        !ir.contains("unhandled preserved"),
        "bare Copy must not trigger the preserved-op fail-loud: {ir}"
    );
}

#[test]
fn lower_preserved_len_ignores_transport_container_type() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend.runtime_callable_symbols.insert("molt_len".into());
    let mut func = TirFunction::new(
        "len_preserved".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let obj = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(obj));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![obj],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("len".into()));
            attrs.insert("container_type".into(), AttrValue::Str("tuple".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("call i64 @molt_len("), "{ir}");
    assert!(!ir.contains("call i64 @molt_len_tuple("), "{ir}");
}

#[test]
fn lower_preserved_len_uses_tir_tuple_fact() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_len_tuple".into());
    let mut func = TirFunction::new(
        "len_typed_tuple".into(),
        vec![TirType::Tuple(vec![TirType::DynBox, TirType::DynBox])],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let obj = ValueId(0);
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![obj],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("len".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("call i64 @molt_len_tuple("), "{ir}");
}

#[test]
fn custom_container_calls_retire_discarded_owned_returns() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend.runtime_callable_symbols.insert("molt_len".into());
    backend
        .runtime_callable_symbols
        .insert("molt_iter_checked".into());
    let mut func = TirFunction::new(
        "container_owned_sinks".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let source = func.fresh_value();
    let unpacked = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(source));
    for kind in ["len", "iter"] {
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![source],
            results: vec![],
            attrs: AttrDict::from([("_original_kind".into(), AttrValue::Str(kind.into()))]),
            source_span: None,
        });
    }
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![source],
        results: vec![unpacked],
        attrs: AttrDict::from([
            (
                "_original_kind".into(),
                AttrValue::Str("unpack_sequence".into()),
            ),
            ("value".into(), AttrValue::Int(1)),
        ]),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![unpacked],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    backend.module.verify().expect("custom container sinks");
    let ir = llvm_fn.print_to_string().to_string();
    for result in ["molt_len", "molt_iter_checked", "unpack_sequence"] {
        assert!(
            ir.contains(&format!("call void @molt_dec_ref_obj(i64 %{result}")),
            "custom result {result} must be retired through its provider-backed ownership contract: {ir}"
        );
    }
}

#[test]
fn custom_container_calls_box_and_retire_raw_inputs() {
    for (kind, symbol) in [("len", "molt_len"), ("iter", "molt_iter_checked")] {
        let ctx = Context::create();
        let mut backend = make_backend(&ctx);
        backend.runtime_callable_symbols.insert(symbol.into());
        let mut func = TirFunction::new(
            "raw_preserved_container".into(),
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
            opcode: OpCode::Copy,
            operands: vec![raw],
            results: vec![],
            attrs: AttrDict::from([("_original_kind".into(), AttrValue::Str(kind.into()))]),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![fallback],
        };

        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        backend
            .module
            .verify()
            .unwrap_or_else(|error| panic!("{kind}: {error}"));
        let ir = llvm_fn.print_to_string().to_string();
        let call = format!("call i64 @{symbol}(i64 %boxed_int)");
        let input_release = "call void @molt_dec_ref_obj(i64 %boxed_call_owner_bits)";
        assert!(
            ir.contains(&call),
            "raw preserved {kind} input must be boxed: {ir}"
        );
        assert!(
            ir.contains(input_release),
            "raw preserved {kind} owner must be retired: {ir}"
        );
        assert!(
            ir.find(&call).unwrap() < ir.find(input_release).unwrap(),
            "{ir}"
        );
        assert!(
            ir.contains("call void @molt_dec_ref_obj(i64 %boxed_call_result)"),
            "discarded preserved {kind} result must be retired: {ir}"
        );
    }

    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "raw_preserved_unpack".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let raw = func.fresh_value();
    let unpacked = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_int_def(raw, i64::MAX));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![raw],
        results: vec![unpacked],
        attrs: AttrDict::from([
            (
                "_original_kind".into(),
                AttrValue::Str("unpack_sequence".into()),
            ),
            ("value".into(), AttrValue::Int(1)),
        ]),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![unpacked],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    backend
        .module
        .verify()
        .expect("raw preserved unpack input ownership");
    let ir = llvm_fn.print_to_string().to_string();
    let call = "call i64 @molt_unpack_sequence(i64 %boxed_int, i64 1, i64 %unpack_out_ptr)";
    let input_release = "call void @molt_dec_ref_obj(i64 %unpack_owner_bits)";
    assert!(
        ir.contains(call),
        "unpack must keep its boxed/raw/raw mixed ABI: {ir}"
    );
    assert!(
        ir.contains(input_release),
        "unpack input temporary must be retired: {ir}"
    );
    assert!(
        ir.find(call).unwrap() < ir.find(input_release).unwrap(),
        "{ir}"
    );
    assert!(
        ir.contains("call void @molt_dec_ref_obj(i64 %unpack_sequence)"),
        "unpack status owner must remain independently retired: {ir}"
    );
    assert!(
        ir.contains("%unpack_elem = load i64, ptr %unpack_elem_ptr"),
        "{ir}"
    );
}

#[test]
fn lower_preserved_list_append_calls_runtime() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_list_append".into());
    let mut func = TirFunction::new(
        "list_append_preserved".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let list_bits = func.fresh_value();
    let item_bits = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(list_bits), const_none_def(item_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![list_bits, item_bits],
        results: vec![],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("list_append".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_list_append"), "{ir}");
}

#[test]
fn lower_del_boundary_calls_dec_ref_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "del_boundary_release".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let owned = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(owned));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DelBoundary,
        operands: vec![owned],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_dec_ref_obj"), "{ir}");
}

#[test]
fn lower_preserved_list_pop_calls_runtime() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_list_pop".to_string());
    let ir = lower_preserved_kind_ir(&backend, "list_pop", 2, true, None)
        .expect("list_pop must lower through the boxed runtime call");
    assert!(ir.contains("molt_list_pop"), "{ir}");
}

#[test]
fn lower_preserved_dataclass_new_values_calls_runtime_with_value_slice() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let ir = lower_preserved_kind_ir(&backend, "dataclass_new_values", 5, true, None)
        .expect("dataclass_new_values must lower through its value-slice runtime call");
    assert!(ir.contains("molt_dataclass_new_from_values"), "{ir}");
    assert!(ir.contains("alloca i64, i64 2"), "{ir}");
}

#[test]
fn preserved_word_ranges_use_static_entry_block_slots() {
    for (kind, operand_count, result_count) in
        [("unpack_sequence", 1, 2), ("dataclass_new_values", 5, 1)]
    {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            format!("{kind}_word_range"),
            vec![],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let operands: Vec<_> = (0..operand_count).map(|_| func.fresh_value()).collect();
        let results: Vec<_> = (0..result_count).map(|_| func.fresh_value()).collect();
        let body = func.fresh_block();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .extend(operands.iter().map(|&operand| const_none_def(operand)));
        entry.terminator = Terminator::Branch {
            target: body,
            args: vec![],
        };
        let mut attrs = AttrDict::from([("_original_kind".into(), AttrValue::Str(kind.into()))]);
        if kind == "unpack_sequence" {
            attrs.insert("value".into(), AttrValue::Int(result_count as i64));
        }
        // An operation in a loop-shaped block reuses one static slot; a range
        // allocated where the operation runs would grow the stack per iteration.
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::Copy,
                    operands,
                    results: results.clone(),
                    attrs,
                    source_span: None,
                }],
                terminator: Terminator::Return {
                    values: vec![results[0]],
                },
            },
        );
        let llvm_fn = lower_tir_to_llvm(&func, &backend);
        backend
            .module
            .verify()
            .expect("preserved word-range lowering must verify");
        let ir = llvm_fn.print_to_string().to_string();
        let entry = llvm_fn.get_first_basic_block().unwrap();
        let mut entry_ir = String::new();
        let mut instruction = entry.get_first_instruction();
        while let Some(current) = instruction {
            entry_ir.push_str(&current.print_to_string().to_string());
            instruction = current.get_next_instruction();
        }
        assert!(entry_ir.contains("alloca i64, i64 2"), "{kind}: {ir}");
        assert_eq!(ir.matches("alloca i64, i64 2").count(), 1, "{kind}: {ir}");
    }
}

#[test]
fn lower_preserved_tuple_from_list_calls_runtime() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_tuple_from_list".into());
    let mut func = TirFunction::new(
        "tuple_from_list_preserved".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let list_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(list_bits));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![list_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("tuple_from_list".into()),
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
    assert!(ir.contains("molt_tuple_from_list"), "{ir}");
}

#[test]
fn lower_preserved_set_add_calls_runtime() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_set_add".into());
    let mut func = TirFunction::new(
        "set_add_preserved".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let set_bits = func.fresh_value();
    let item_bits = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(set_bits), const_none_def(item_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![set_bits, item_bits],
        results: vec![],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("set_add".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_set_add"), "{ir}");
}

#[test]
fn lower_preserved_list_extend_calls_runtime() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_list_extend".into());
    let mut func = TirFunction::new(
        "list_extend_preserved".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let list_bits = func.fresh_value();
    let other_bits = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(list_bits), const_none_def(other_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![list_bits, other_bits],
        results: vec![],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("list_extend".into()),
            );
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_list_extend"), "{ir}");
}

#[test]
fn lower_preserved_aiter_calls_runtime() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend.runtime_callable_symbols.insert("molt_aiter".into());
    let mut func = TirFunction::new(
        "aiter_preserved".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let obj_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(const_none_def(obj_bits));
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![obj_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("aiter".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_aiter"), "{ir}");
}

#[test]
fn lower_preserved_gen_send_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "gen_send_preserved".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let gen_bits = func.fresh_value();
    let send_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(gen_bits), const_none_def(send_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![gen_bits, send_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("gen_send".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_generator_send"), "{ir}");
}

#[test]
fn lower_preserved_sys_executable_uses_classified_runtime_abi() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_sys_executable".to_string());
    let mut func = TirFunction::new(
        "sys_executable_preserved".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("sys_executable".into()),
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
    assert!(ir.contains("call i64 @molt_sys_executable()"), "{ir}");
}

#[test]
fn lower_preserved_context_exit_calls_runtime() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_context_exit".into());
    let mut func = TirFunction::new(
        "context_exit_preserved".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let ctx_bits = func.fresh_value();
    let exc_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(ctx_bits), const_none_def(exc_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![ctx_bits, exc_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert(
                "_original_kind".into(),
                AttrValue::Str("context_exit".into()),
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
    assert!(ir.contains("molt_context_exit"), "{ir}");
}

#[test]
fn lower_preserved_super_new_calls_runtime() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new(
        "super_new_preserved".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let type_bits = func.fresh_value();
    let obj_bits = func.fresh_value();
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry
        .ops
        .extend([const_none_def(type_bits), const_none_def(obj_bits)]);
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![type_bits, obj_bits],
        results: vec![result],
        attrs: {
            let mut attrs = AttrDict::new();
            attrs.insert("_original_kind".into(), AttrValue::Str("super_new".into()));
            attrs
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };

    let llvm_fn = lower_tir_to_llvm(&func, &backend);
    let ir = llvm_fn.print_to_string().to_string();
    assert!(ir.contains("molt_super_new"), "{ir}");
}

#[test]
fn runtime_guard_results_return_the_original_operand_after_checking() {
    for kind in ["guard_tag", "guard_type"] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            format!("checked_alias_{kind}"),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let source = entry.args[0].id;
        let tag = entry.args[1].id;
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![source, tag],
            results: vec![result],
            attrs: AttrDict::from([("_original_kind".into(), AttrValue::Str(kind.into()))]),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let ir = try_lower_tir_to_llvm(&func, &backend)
            .expect("guard lowering")
            .print_to_string()
            .to_string();
        assert!(ir.contains("@molt_guard_type(i64 %0, i64 %1)"), "{ir}");
        assert!(
            ir.contains("ret i64 %0"),
            "guard must return its source: {ir}"
        );
    }
}

#[test]
fn runtime_guard_literal_tag_loads_one_entry_profile_flag_and_keeps_source() {
    for kind in ["guard_tag", "guard_type"] {
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let mut func = TirFunction::new(
            format!("profile_guard_{kind}"),
            vec![TirType::DynBox],
            TirType::DynBox,
            molt_ir::FunctionReturnAbi::Value,
        );
        let tag = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let source = entry.args[0].id;
        entry.ops.push(const_int_def(tag, 5));
        for _ in 0..2 {
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Copy,
                operands: vec![source, tag],
                results: vec![],
                attrs: AttrDict::from([("_original_kind".into(), AttrValue::Str(kind.into()))]),
                source_span: None,
            });
        }
        entry.ops.last_mut().unwrap().results = vec![result];
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let llvm = try_lower_tir_to_llvm(&func, &backend).expect("profile guard lowering");
        assert!(llvm.verify(true));
        let ir = llvm.print_to_string().to_string();
        assert_eq!(
            ir.matches("call i64 @molt_profile_enabled()").count(),
            1,
            "{ir}"
        );
        assert_eq!(ir.matches("call i64 @molt_guard_type(").count(), 2, "{ir}");
        assert!(
            ir.contains("guard_profile") && ir.contains("ret i64 %0"),
            "{ir}"
        );
    }
}
