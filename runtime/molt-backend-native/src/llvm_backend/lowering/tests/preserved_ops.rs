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
        let mut func = TirFunction::new("builtin_arguments".into(), vec![], TirType::DynBox);
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
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend.function_linkage_abis.insert(
        "gen_fn".to_string(),
        test_native_linkage_abi(vec![], Some(TirType::DynBox)),
    );
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
            "exception_set_cause",
            2,
            false,
            None,
            "molt_exception_set_cause",
        ),
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
            "gen_locals_register",
            2,
            false,
            Some("gen_fn"),
            "molt_gen_locals_register",
        ),
        (
            "asyncgen_locals_register",
            2,
            false,
            Some("gen_fn"),
            "molt_asyncgen_locals_register",
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
            backend.function_linkage_abis.insert(
                "callable_result_target".into(),
                test_native_linkage_abi(
                    if kind == "func_new_closure" {
                        vec![TirType::DynBox]
                    } else {
                        vec![]
                    },
                    Some(TirType::DynBox),
                ),
            );
            let symbol_target = matches!(kind, "func_new" | "func_new_closure" | "builtin_func")
                .then_some("callable_result_target");
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
        let mut func = TirFunction::new(format!("boxed_{kind}"), vec![], TirType::DynBox);
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
            ir.matches("call void @molt_dec_ref_obj(i64 %boxed_int")
                .count(),
            arity,
            "{ir}"
        );
        assert_eq!(
            ir.matches(&format!("phi i64 [ {boxed}, %box_int_inline"))
                .count(),
            arity,
            "{ir}"
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
fn direct_and_preserved_boxed_calls_share_result_custody() {
    for opcode in [OpCode::Call, OpCode::Copy] {
        for (kind, symbol, arity, shape) in [
            ("dict_set", "molt_dict_set", 3, "borrowed"),
            (
                "dict_update_missing",
                "molt_dict_update_missing",
                3,
                "borrowed",
            ),
            ("list_append", "molt_list_append", 2, "owned"),
            ("spawn", "molt_spawn", 1, "void"),
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
            3,
            "each raw argument must be materialized independently: {ir}"
        );
        assert_eq!(
            ir.matches("call void @molt_dec_ref_obj(i64 %boxed_int")
                .count(),
            3,
            "each temporary boxed argument owner must be retired: {ir}"
        );
        let retain = ir
            .find("call void @molt_inc_ref_obj(i64 %molt_dict_set)")
            .unwrap_or_else(|| panic!("bound borrowed result was not retained: {ir}"));
        let cleanup = ir
            .find("call void @molt_dec_ref_obj(i64 %boxed_int")
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
        let ctx = Context::create();
        let backend = make_backend(&ctx);
        let func = runtime_call_shape_function(opcode, "dict_set", "molt_dict_set", 3, true, true);
        let error = try_lower_tir_to_llvm(&func, &backend)
            .expect_err("generated semantics must not imply runtime availability");
        assert_lowering_error_contains(
            &error,
            "boxed runtime symbol `molt_dict_set` is unavailable in the selected runtime",
        );
        assert!(backend.module.get_function("molt_dict_set").is_none());
        let ir = backend.module.print_to_string().to_string();
        assert!(!ir.contains("call i64 @molt_int_from_i64("), "{ir}");
    }
}

#[test]
fn generator_locals_registration_preserves_mixed_abi_for_both_families() {
    for kind in ["gen_locals_register", "asyncgen_locals_register"] {
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
        );
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let names = entry.args[0].id;
        let offsets = entry.args[1].id;
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Copy,
            operands: vec![names, offsets],
            results: vec![result],
            attrs: AttrDict::from([
                ("_original_kind".into(), AttrValue::Str(kind.into())),
                ("s_value".into(), AttrValue::Str("poll_fn".into())),
                ("value".into(), AttrValue::Int(1)),
            ]),
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
    let mut func = TirFunction::new("marked_finally_observer".into(), vec![], TirType::DynBox);
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

/// RESULT-LESS preserved side-effect ops (`print_newline`, `set_update`,
/// `dict_str_int_inc`, …) whose `molt_<kind>` symbol IS in the linked
/// intrinsic surface must lower to that runtime call via the generic
/// fallback — NOT be dropped as a `Copy` "0 results → no-op". The
/// passthrough enumeration found these reaching the no-op branch (a missing
/// newline / a set or dict mutation that never happened). This pins the
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
        ("dict_str_int_inc", 3, "molt_dict_str_int_inc"),
        ("spawn", 1, "molt_spawn"),
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
fn lower_preserved_chan_new_uses_dedicated_handle_lowering() {
    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let ir = lower_preserved_kind_ir(&backend, "chan_new", 1, true, None).unwrap_or_else(|e| {
        panic!(
            "chan_new returns an opaque channel handle and must lower through \
             its dedicated LLVM arm, got error: {:?}",
            e.diagnostics()
        )
    });
    assert!(
        ir.contains("call i64 @molt_chan_new(i64"),
        "chan_new must call the centrally declared handle constructor; IR:\n{ir}"
    );
}

#[test]
fn lower_preserved_void_runtime_result_shape_fails_loud() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_spawn".to_string());
    let err = lower_preserved_kind_ir(&backend, "spawn", 1, true, None)
        .expect_err("void preserved runtime ops must not bind a boxed result");
    assert_lowering_error_contains(&err, "call to void runtime symbol");
    assert_lowering_error_contains(&err, "spawn");
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
    let mut func = TirFunction::new("bare_copy".into(), vec![], TirType::DynBox);
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
    let mut func = TirFunction::new("len_preserved".into(), vec![], TirType::DynBox);
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
    let mut func = TirFunction::new("container_owned_sinks".into(), vec![], TirType::DynBox);
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
        let mut func = TirFunction::new("raw_preserved_container".into(), vec![], TirType::DynBox);
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
        let input_release = "call void @molt_dec_ref_obj(i64 %boxed_int)";
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
            ir.contains(&format!("call void @molt_dec_ref_obj(i64 %{symbol})")),
            "discarded preserved {kind} result must be retired: {ir}"
        );
    }

    let ctx = Context::create();
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("raw_preserved_unpack".into(), vec![], TirType::DynBox);
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
    let input_release = "call void @molt_dec_ref_obj(i64 %boxed_int)";
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
    let mut func = TirFunction::new("list_append_preserved".into(), vec![], TirType::DynBox);
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
    let mut func = TirFunction::new("del_boundary_release".into(), vec![], TirType::DynBox);
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
fn lower_preserved_tuple_from_list_calls_runtime() {
    let ctx = Context::create();
    let mut backend = make_backend(&ctx);
    backend
        .runtime_callable_symbols
        .insert("molt_tuple_from_list".into());
    let mut func = TirFunction::new("tuple_from_list_preserved".into(), vec![], TirType::DynBox);
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
    let mut func = TirFunction::new("set_add_preserved".into(), vec![], TirType::DynBox);
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
    let mut func = TirFunction::new("list_extend_preserved".into(), vec![], TirType::DynBox);
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
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("aiter_preserved".into(), vec![], TirType::DynBox);
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
    let mut func = TirFunction::new("gen_send_preserved".into(), vec![], TirType::DynBox);
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
    let mut func = TirFunction::new("sys_executable_preserved".into(), vec![], TirType::DynBox);
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
    let backend = make_backend(&ctx);
    let mut func = TirFunction::new("context_exit_preserved".into(), vec![], TirType::DynBox);
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
    let mut func = TirFunction::new("super_new_preserved".into(), vec![], TirType::DynBox);
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
