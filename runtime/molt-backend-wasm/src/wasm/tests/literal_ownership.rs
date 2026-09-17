use super::support::*;

fn compile_literal_body(params: Vec<&str>, ops: Vec<OpIR>) -> (Vec<String>, BTreeMap<String, u32>) {
    let ir = SimpleIR {
        functions: vec![wasm_test_function("molt_main", params, None, ops)],
        profile: None,
    };
    let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(ir);
    (
        wasm_operator_debug_for_export(&output.wasm, "molt_main"),
        wasm_function_import_indices(&output.wasm),
    )
}

fn call_count(operators: &[String], function_index: u32) -> usize {
    let call = format!("Call {{ function_index: {function_index} }}");
    operators
        .iter()
        .filter(|operator| operator.as_str() == call.as_str())
        .count()
}

#[test]
fn direct_calls_release_only_owned_value_results() {
    for (kind, target, argc, owns_result, returns_value) in [
        ("call", "molt_classmethod_new", 1, true, true),
        ("call", "molt_function_closure_bits", 1, false, true),
        ("call", "molt_print_newline", 0, false, false),
        ("call", "owned_call_target", 1, true, true),
        ("call_internal", "owned_call_target", 1, true, true),
        ("call", "external_owned_target", 1, true, true),
        ("call_internal", "external_owned_target", 1, true, true),
        ("call_internal", "external_void_target", 0, false, false),
    ] {
        for bound in [false, true] {
            if target == "molt_print_newline" && bound {
                continue;
            }
            let mut call = wasm_test_op(kind, bound.then_some("result"), vec!["value"; argc]);
            call.s_value = Some(target.into());
            let mut external_owned = wasm_test_function(
                "external_owned_target",
                vec!["arg"],
                None,
                vec![wasm_test_op("ret", None, vec!["arg"])],
            );
            external_owned.externalize_with_signature().unwrap();
            let mut external_void = wasm_test_function(
                "external_void_target",
                vec![],
                None,
                vec![wasm_test_op("ret_void", None, vec![])],
            );
            external_void.externalize_with_signature().unwrap();
            let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
                functions: vec![
                    wasm_test_function(
                        "molt_main",
                        vec!["value"],
                        None,
                        vec![
                            call,
                            wasm_test_op("const_none", Some("nothing"), vec![]),
                            wasm_test_op(
                                "ret",
                                None,
                                vec![if bound { "result" } else { "nothing" }],
                            ),
                        ],
                    ),
                    wasm_test_function(
                        "owned_call_target",
                        vec!["arg"],
                        None,
                        vec![wasm_test_op("ret", None, vec!["arg"])],
                    ),
                    external_owned,
                    external_void,
                ],
                profile: None,
            });
            wasmparser::Validator::new()
                .validate_all(&output.wasm)
                .unwrap_or_else(|error| panic!("{kind} {target}, bound={bound}: {error}"));
            let operators = wasm_operator_debug_for_export(&output.wasm, "molt_main");
            let imports = wasm_function_import_indices(&output.wasm);
            let releases = imports
                .get("dec_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(
                releases,
                usize::from(owns_result && !bound),
                "{kind} {target}, bound={bound}: {operators:?}"
            );
            let retains = imports
                .get("inc_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(retains, 0, "{kind} {target}, bound={bound}: {operators:?}");
            if !owns_result && returns_value && !bound {
                let target_import = imports["function_closure_bits"];
                let position = operators
                    .iter()
                    .position(|operator| {
                        operator == &format!("Call {{ function_index: {target_import} }}")
                    })
                    .unwrap();
                assert_eq!(operators[position + 1], "Drop", "{operators:?}");
            }
        }
    }
}

#[test]
fn dynamic_calls_release_discarded_owned_results() {
    for (kind, argc, target) in [
        ("call_func", 1, None),
        ("call_bind", 2, None),
        ("call_indirect", 2, None),
        ("call_guarded", 2, Some("owned_call_target")),
        ("call_method", 1, None),
        ("call_method", 1, Some("BoundMethod:str:upper")),
        ("call_method_ic", 1, Some("method")),
        ("call_super_method_ic", 2, Some("method")),
        ("invoke_ffi", 1, None),
    ] {
        for bound in [false, true] {
            let mut call = wasm_test_op(kind, bound.then_some("result"), vec!["value"; argc]);
            call.s_value = target.map(str::to_string);
            let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
                functions: vec![
                    wasm_test_function(
                        "molt_main",
                        vec!["value"],
                        None,
                        vec![
                            call,
                            wasm_test_op("const_none", Some("nothing"), vec![]),
                            wasm_test_op(
                                "ret",
                                None,
                                vec![if bound { "result" } else { "nothing" }],
                            ),
                        ],
                    ),
                    wasm_test_function(
                        "owned_call_target",
                        vec!["arg"],
                        None,
                        vec![wasm_test_op("ret", None, vec!["arg"])],
                    ),
                ],
                profile: None,
            });
            wasmparser::Validator::new()
                .validate_all(&output.wasm)
                .unwrap_or_else(|error| panic!("{kind} {target:?}, bound={bound}: {error}"));
            let operators = wasm_operator_debug_for_export(&output.wasm, "molt_main");
            let imports = wasm_function_import_indices(&output.wasm);
            let releases = imports
                .get("dec_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(
                releases,
                usize::from(!bound),
                "{kind} {target:?}, bound={bound}: {operators:?}"
            );
        }
    }
}

#[test]
fn native_symbol_results_preserve_owned_and_raw_abis() {
    for (abi, argc, owns_result) in [
        ("molt.object_call_v1", 1, true),
        ("molt.object_callargs_v1", 1, true),
        ("molt.forward_f32_v1", 1, true),
        ("molt.pyinit_module_v1", 0, false),
    ] {
        for bound in [false, true] {
            let mut call =
                wasm_test_op("invoke_ffi", bound.then_some("result"), vec!["value"; argc]);
            call.native_callable_export = Some("native.probe".into());
            call.native_callable_binding = Some("direct_symbol".into());
            call.native_callable_symbol = Some("native_probe".into());
            call.native_callable_abi = Some(abi.into());
            let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
                functions: vec![wasm_test_function(
                    "molt_main",
                    vec!["value"],
                    None,
                    vec![
                        call,
                        wasm_test_op("const_none", Some("nothing"), vec![]),
                        wasm_test_op("ret", None, vec![if bound { "result" } else { "nothing" }]),
                    ],
                )],
                profile: None,
            });
            wasmparser::Validator::new()
                .validate_all(&output.wasm)
                .unwrap_or_else(|error| panic!("{abi}, bound={bound}: {error}"));
            let operators = wasm_operator_debug_for_export(&output.wasm, "molt_main");
            let imports = wasm_function_import_indices(&output.wasm);
            let releases = imports
                .get("dec_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(
                releases,
                usize::from(owns_result && !bound),
                "{abi}, bound={bound}: {operators:?}"
            );
        }
    }
}

#[test]
fn direct_calls_reject_runtime_void_outputs_and_internal_runtime_targets() {
    for (kind, target, argc) in [
        ("call", "molt_print_newline", 0),
        ("call_internal", "molt_classmethod_new", 1),
    ] {
        let outcome = std::panic::catch_unwind(|| {
            let mut call = wasm_test_op(kind, Some("result"), vec!["arg"; argc]);
            call.s_value = Some(target.into());
            wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(SimpleIR {
                functions: vec![wasm_test_function(
                    "molt_main",
                    vec!["arg"],
                    None,
                    vec![call, wasm_test_op("ret", None, vec!["result"])],
                )],
                profile: None,
            });
        });
        assert!(
            outcome.is_err(),
            "{kind} {target} must reject the invalid result ABI"
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
            let mut constructor =
                wasm_test_op(kind, bound.then_some("created"), vec!["value"; argc]);
            if matches!(kind, "func_new" | "func_new_closure" | "builtin_func") {
                constructor.s_value = Some("callable_result_target".into());
                constructor.value = Some(0);
            }
            let ir = SimpleIR {
                functions: vec![
                    wasm_test_function(
                        "molt_main",
                        vec!["value"],
                        None,
                        vec![
                            constructor,
                            wasm_test_op(
                                "ret",
                                None,
                                vec![if bound { "created" } else { "value" }],
                            ),
                        ],
                    ),
                    wasm_test_function(
                        "callable_result_target",
                        if kind == "func_new_closure" {
                            vec!["closure"]
                        } else {
                            vec![]
                        },
                        None,
                        vec![
                            wasm_test_op("const_none", Some("nothing"), vec![]),
                            wasm_test_op("ret", None, vec!["nothing"]),
                        ],
                    ),
                ],
                profile: None,
            };
            let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(ir);
            wasmparser::Validator::new()
                .validate_all(&output.wasm)
                .unwrap_or_else(|error| panic!("{kind}, bound={bound}: {error}"));
            let operators = wasm_operator_debug_for_export(&output.wasm, "molt_main");
            let imports = wasm_function_import_indices(&output.wasm);
            let symbol = if kind == "builtin_func" {
                "func_new_builtin"
            } else {
                kind
            };
            assert_eq!(
                call_count(&operators, imports[symbol]),
                1,
                "{kind}: {operators:?}"
            );
            let releases = imports
                .get("dec_ref_obj")
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(
                releases,
                usize::from(!bound),
                "{kind}, bound={bound}: {operators:?}"
            );
            if !bound {
                let call = format!("Call {{ function_index: {} }}", imports[symbol]);
                let position = operators
                    .iter()
                    .position(|operator| operator == &call)
                    .unwrap();
                assert_eq!(
                    operators[position + 1],
                    format!("Call {{ function_index: {} }}", imports["dec_ref_obj"]),
                    "{operators:?}"
                );
            }
        }
    }
}

#[test]
fn closure_extraction_retains_only_bound_borrowed_results() {
    for bound in [false, true] {
        let ir = SimpleIR {
            functions: vec![wasm_test_function(
                "molt_main",
                vec!["callee"],
                None,
                vec![
                    wasm_test_op(
                        "function_closure_bits",
                        bound.then_some("closure"),
                        vec!["callee"],
                    ),
                    if bound {
                        wasm_test_op("ret", None, vec!["closure"])
                    } else {
                        wasm_test_op("ret_void", None, vec![])
                    },
                ],
            )],
            profile: None,
        };
        let output = wasm_compile_final_ir_for_op_loop_tests_with_diagnostics(ir);
        wasmparser::Validator::new()
            .validate_all(&output.wasm)
            .expect("bound and discarded borrowed results must produce valid WASM");
        let operators = wasm_operator_debug_for_export(&output.wasm, "molt_main");
        let imports = wasm_function_import_indices(&output.wasm);
        let extract = imports["function_closure_bits"];
        assert_eq!(call_count(&operators, extract), 1, "{operators:?}");
        for (symbol, expected) in [
            ("inc_ref_obj", usize::from(bound)),
            ("dec_ref_obj", 0),
            ("dec_ref", 0),
        ] {
            let actual = imports
                .get(symbol)
                .map_or(0, |&index| call_count(&operators, index));
            assert_eq!(actual, expected, "bound={bound}: {symbol}: {operators:?}");
        }
        let extract_call = format!("Call {{ function_index: {extract} }}");
        let position = operators.iter().position(|op| op == &extract_call).unwrap();
        if bound {
            assert!(
                operators[position + 1].starts_with("LocalSet {"),
                "{operators:?}"
            );
            assert!(
                operators[position + 2].starts_with("LocalGet {"),
                "{operators:?}"
            );
            assert_eq!(
                operators[position + 3],
                format!("Call {{ function_index: {} }}", imports["inc_ref_obj"]),
                "{operators:?}"
            );
        } else {
            assert_eq!(operators[position + 1], "Drop", "{operators:?}");
        }
    }
}

pub(super) fn assert_every_exit_releases_anchor(
    operators: &[String],
    dec_ref_index: u32,
    expected_minimum_returns: usize,
) -> usize {
    let release = format!("Call {{ function_index: {dec_ref_index} }}");
    let mut exit_positions: Vec<usize> = operators
        .iter()
        .enumerate()
        .filter_map(|(index, operator)| (operator == "Return").then_some(index))
        .collect();
    assert!(
        exit_positions.len() >= expected_minimum_returns,
        "expected at least {expected_minimum_returns} return paths; operators={operators:?}"
    );
    assert_eq!(operators.last().map(String::as_str), Some("End"));
    // Plain bodies include an implicit fallthrough epilogue even when the IR
    // ends with an explicit return. Dispatch bodies end in an explicit Return.
    if operators
        .get(operators.len().wrapping_sub(2))
        .map(String::as_str)
        != Some("Return")
    {
        exit_positions.push(operators.len() - 1);
    }
    for &return_index in &exit_positions {
        assert_eq!(
            operators.get(return_index.wrapping_sub(1)),
            Some(&release),
            "every explicit or implicit function exit must release its unique anchor immediately before returning; operators={operators:?}"
        );
    }
    exit_positions.len()
}

#[test]
fn jumpful_literals_share_one_anchor_and_mint_each_dynamic_result_owner() {
    let mut first = wasm_test_op("const_str", Some("first"), vec![]);
    first.s_value = Some("shared-payload".to_string());
    let mut branch = wasm_test_op("br_if", None, vec!["cond"]);
    branch.value = Some(7);
    let mut second = wasm_test_op("const_str", Some("second"), vec![]);
    second.s_value = Some("shared-payload".to_string());
    let mut label = wasm_test_op("label", None, vec![]);
    label.value = Some(7);

    let (operators, imports) = compile_literal_body(
        vec!["cond"],
        vec![
            first,
            wasm_test_op("dec_ref", None, vec!["first"]),
            branch,
            second,
            wasm_test_op("dec_ref", None, vec!["second"]),
            label,
            wasm_test_op("ret_void", None, vec![]),
        ],
    );

    assert_eq!(call_count(&operators, imports["string_from_bytes"]), 1);
    assert_eq!(call_count(&operators, imports["exception_pending"]), 1);
    assert_eq!(
        call_count(&operators, imports["inc_ref_obj"]),
        2,
        "each original literal op must mint its own result owner from the shared anchor; operators={operators:?}"
    );
    assert_every_exit_releases_anchor(&operators, imports["dec_ref_obj"], 2);
}

#[test]
fn full_i64_const_uses_fallible_anchor_instead_of_inline_47_truncation() {
    let mut wide = wasm_test_op("const", Some("wide"), vec![]);
    wide.value = Some(i64::MAX);
    let (operators, imports) = compile_literal_body(
        vec![],
        vec![
            wide,
            wasm_test_op("dec_ref", None, vec!["wide"]),
            wasm_test_op("ret_void", None, vec![]),
        ],
    );

    assert_eq!(call_count(&operators, imports["int_from_i64"]), 1);
    assert_eq!(call_count(&operators, imports["exception_pending"]), 1);
    assert_eq!(call_count(&operators, imports["inc_ref_obj"]), 1);
    assert!(
        operators
            .iter()
            .any(|operator| operator == &format!("I64Const {{ value: {} }}", i64::MAX)),
        "full-width constant must reach int_from_i64 without a 47-bit mask; operators={operators:?}"
    );
    assert_every_exit_releases_anchor(&operators, imports["dec_ref_obj"], 2);
}
