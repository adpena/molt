use super::*;

#[test]
fn test_compile_checked_lowers_checked_add_helper() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "checked_add_test".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "checked_add".to_string(),
                    args: Some(vec!["a".to_string(), "b".to_string()]),
                    var: Some("sum".to_string()),
                    out: Some("overflow".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["sum".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);

    assert!(source.contains("function molt_checked_i64_add"));
    assert!(source.contains("return a + b, false"));
    assert!(source.contains("local sum: number, overflow: boolean = molt_checked_i64_add(a, b)"));
    assert!(!source.contains("[unsupported op: checked_add]"));
}

#[test]
fn test_compile_checked_lowers_checked_mul_helper() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "checked_mul_test".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "checked_mul".to_string(),
                    args: Some(vec!["a".to_string(), "b".to_string()]),
                    var: Some("product".to_string()),
                    out: Some("overflow".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["product".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);

    assert!(source.contains("function molt_checked_i64_mul"));
    assert!(source.contains("if p >= 9007199254740992 or p <= -9007199254740992"));
    assert!(
        source.contains("local product: number, overflow: boolean = molt_checked_i64_mul(a, b)")
    );
    assert!(!source.contains("[unsupported op: checked_mul]"));
}

#[test]
fn test_checked_numeric_results_preserve_discarded_field_positions() {
    for (kind, helper) in [
        ("checked_add", "molt_checked_i64_add"),
        ("checked_mul", "molt_checked_i64_mul"),
    ] {
        for discarded in [None, Some("none")] {
            let compile = |var: Option<&str>, out: Option<&str>| {
                let ir = SimpleIR {
                    functions: vec![FunctionIR {
                        return_abi: molt_ir::FunctionReturnAbi::Void,
                        name: format!("{kind}_discarded_result_test"),
                        params: vec!["a".to_string(), "b".to_string()],
                        param_types: Some(vec!["int".to_string(), "int".to_string()]),
                        source_file: None,
                        is_extern: false,
                        codegen_partition: false,
                        parameter_custody: Vec::new(),
                        execution_context: ExecutionContextPolicy::None,
                        ops: vec![
                            OpIR {
                                kind: kind.to_string(),
                                args: Some(vec!["a".to_string(), "b".to_string()]),
                                var: var.map(str::to_string),
                                out: out.map(str::to_string),
                                ..OpIR::default()
                            },
                            OpIR {
                                kind: "ret_void".to_string(),
                                ..OpIR::default()
                            },
                        ],
                    }],
                    profile: None,
                };
                LuauBackend::new().compile(&ir)
            };

            let source = compile(discarded, Some("overflow"));
            assert!(source.contains(&format!("local _, overflow: boolean = {helper}(a, b)")));
            assert!(!source.contains(&format!("local overflow: number = {helper}(a, b)")));

            let source = compile(Some("value"), discarded);
            assert!(source.contains(&format!("local value: number = {helper}(a, b)")));
            assert!(!source.contains(&format!("local _, value: boolean = {helper}(a, b)")));
        }
    }
}

#[test]
fn test_compile_checked_lowers_zero_division_guards() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "zero_division_guard_test".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "div".to_string(),
                    args: Some(vec!["a".to_string(), "b".to_string()]),
                    out: Some("quotient".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "mod".to_string(),
                    args: Some(vec!["a".to_string(), "b".to_string()]),
                    out: Some("remainder".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "floordiv".to_string(),
                    args: Some(vec!["a".to_string(), "b".to_string()]),
                    out: Some("floor_quotient".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["floor_quotient".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);

    assert!(source.contains("if b == 0 then molt_numeric_error(\"truediv:int\") end"));
    assert!(source.contains("if b == 0 then molt_numeric_error(\"mod:int\") end"));
    assert!(source.contains("if b == 0 then molt_numeric_error(\"floordiv:int\") end"));
    assert!(source.contains("[13]=\"float modulo by zero\""));
    assert!(source.contains("[14]=\"division by zero\""));
    assert!(source.contains("local quotient: number = a / b"));
    assert!(source.contains("local remainder: number = a % b"));
    assert!(source.contains("local floor_quotient: number = a // b"));
    assert!(!source.contains("[unsupported op: div]"));
    assert!(!source.contains("[unsupported op: mod]"));
    assert!(!source.contains("[unsupported op: floordiv]"));

    let mut typed_ir = ir;
    typed_ir.functions[0].param_types = Some(vec!["float".to_string(), "float".to_string()]);
    let float_source = LuauBackend::new().compile(&typed_ir);
    assert!(float_source.contains("if b == 0 then molt_numeric_error(\"truediv:float\") end"));
    assert!(float_source.contains("if b == 0 then molt_numeric_error(\"floordiv:float\") end"));
    assert!(float_source.contains("if b == 0 then molt_numeric_error(\"mod:float\") end"));
    typed_ir.functions[0].param_types = Some(vec!["int".to_string(), "bool".to_string()]);
    for operation in &mut typed_ir.functions[0].ops {
        if ["div", "mod", "floordiv"].contains(&operation.kind.as_str()) {
            operation.kind = format!("inplace_{}", operation.kind);
        }
    }
    let bool_source = LuauBackend::new().compile(&typed_ir);
    assert!(
        bool_source
            .contains("if (if b then 1 else 0) == 0 then molt_numeric_error(\"truediv:int\") end")
    );
    assert!(
        bool_source
            .contains("if (if b then 1 else 0) == 0 then molt_numeric_error(\"mod:int\") end")
    );
    assert!(
        bool_source
            .contains("if (if b then 1 else 0) == 0 then molt_numeric_error(\"floordiv:int\") end")
    );
}

#[test]
fn test_compile_checked_lowers_pow_mod_square_multiply_loop() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "pow_mod_test".to_string(),
            params: vec!["base".to_string(), "exp".to_string(), "modulus".to_string()],
            param_types: Some(vec![
                "int".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "pow_mod".to_string(),
                    args: Some(vec![
                        "base".to_string(),
                        "exp".to_string(),
                        "modulus".to_string(),
                    ]),
                    out: Some("result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["result".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);

    assert!(source.contains("local result; do local __b, __e, __m = base % modulus, exp, modulus"));
    assert!(source.contains("while __e > 0 do"));
    assert!(source.contains("__r = (__r * __b) % __m"));
    assert!(source.contains("__e = __e // 2"));
    assert!(!source.contains("[unsupported op: pow_mod]"));
}

fn fused_kernel_function(name: &str, kind: &str, args: &[&str]) -> FunctionIR {
    FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: name.to_string(),
        params: args.iter().map(|arg| arg.to_string()).collect(),
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: ExecutionContextPolicy::None,
        ops: vec![
            OpIR {
                kind: kind.to_string(),
                args: Some(args.iter().map(|arg| arg.to_string()).collect()),
                out: Some(format!("{name}_result")),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec![format!("{name}_result")]),
                ..OpIR::default()
            },
        ],
    }
}

#[test]
fn test_compile_checked_fused_reductions_decline_to_the_ordinary_loop() {
    let args = ["it", "acc", "target"];
    let ir = SimpleIR {
        functions: ["vec_sum", "vec_prod", "vec_min", "vec_max"]
            .iter()
            .map(|kind| fused_kernel_function(kind, kind, &args))
            .collect(),
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);
    for kind in ["vec_sum", "vec_prod", "vec_min", "vec_max"] {
        assert!(
            source.contains(&format!("local {kind}_result = {{nil, nil, 0, false}}")),
            "{kind} must decline, got:\n{source}"
        );
        assert!(!source.contains(&format!("[unsupported op: {kind}]")));
    }
}

#[test]
fn test_compile_checked_fused_split_count_declines_to_the_ordinary_loop() {
    let ir = SimpleIR {
        functions: vec![
            fused_kernel_function(
                "ws",
                "string_split_ws_dict_inc",
                &["line", "dict", "delta", "target"],
            ),
            fused_kernel_function(
                "sep",
                "string_split_sep_dict_inc",
                &["line", "sep", "dict", "delta", "target"],
            ),
        ],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);
    assert!(source.contains("local ws_result = {nil, false}"));
    assert!(source.contains("local sep_result = {nil, false}"));
    assert!(!source.contains("function molt_string_split_ws_dict_inc"));
    assert!(!source.contains("function molt_string_split_sep_dict_inc"));
    assert!(!source.contains("[unsupported op: string_split_ws_dict_inc]"));
    assert!(!source.contains("[unsupported op: string_split_sep_dict_inc]"));
}

#[test]
fn test_compile_checked_lowers_labeled_branch_ops() {
    let branch_function = |name: &str, kind: &str, label: i64, flag_value: i64| FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: name.to_string(),
        params: Vec::new(),
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: ExecutionContextPolicy::None,
        ops: vec![
            OpIR {
                kind: "const_bool".to_string(),
                value: Some(flag_value),
                out: Some("flag".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: kind.to_string(),
                value: Some(label),
                args: Some(vec!["flag".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                value: Some(0),
                out: Some("zero".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["zero".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(label),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                value: Some(1),
                out: Some("one".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["one".to_string()]),
                ..OpIR::default()
            },
        ],
    };
    let ir = SimpleIR {
        functions: vec![
            branch_function("br_if_test", "br_if", 7, 1),
            branch_function("branch_test", "branch", 8, 1),
            branch_function("branch_false_test", "branch_false", 9, 0),
        ],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);

    assert!(source.contains("br_if_test = function()"));
    assert!(source.contains("branch_test = function()"));
    assert!(source.contains("branch_false_test = function()"));
    assert!(!source.contains("[unsupported op: br_if"));
    assert!(!source.contains("[unsupported op: branch "));
    assert!(!source.contains("[unsupported op: branch_false"));
}

#[test]
fn test_compile_via_ir_rejects_unsupported_output() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "unsupported_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![OpIR {
                kind: "unknown_luau_op".to_string(),
                out: Some("v0".to_string()),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let err = backend
        .compile_via_ir(&ir)
        .expect_err("preview/IR path must reject unsupported output");
    assert!(
        err.contains("rejected before source generation")
            && err.contains("`unknown_luau_op`")
            && err.contains("unclassified"),
        "diagnostic must name the unclassified op at pre-source admission, got: {err}"
    );
}

/// Unsupported sink operations fail at the same Result boundary as operations
/// with outputs; neither path emits a substitute value.
#[test]
fn test_compile_via_ir_fails_closed_without_emitted_value_line() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_main".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![OpIR {
                // No `out`: dispatch still records the unsupported operation.
                kind: "molt_synthetic_unsupported_sink_probe".to_string(),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let err = backend
        .compile_via_ir(&ir)
        .expect_err("an unsupported op with no output must still fail closed");
    assert!(
        err.contains("rejected before source generation") && err.contains("unclassified"),
        "got: {err}"
    );
    assert!(
        err.contains("`molt_synthetic_unsupported_sink_probe`"),
        "got: {err}"
    );
}

#[test]
fn test_compile_checked_rejects_malformed_callable_family_without_nil_values() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "molt_main".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "call".to_string(),
                    out: Some("call_result".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "func_new".to_string(),
                    out: Some("function".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "builtin_func".to_string(),
                    s_value: Some("molt_open_builtin".to_string()),
                    out: Some("builtin".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_bind".to_string(),
                    out: Some("bound_result".to_string()),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };

    let mut backend = LuauBackend::new();
    let err = backend
        .compile_checked(&ir)
        .expect_err("malformed callable IR must fail before source publication");
    assert!(
        err.contains("refuses to emit fail-open codegen")
            && err.contains("`call`")
            && err.contains("`func_new`")
            && err.contains("`builtin_func`")
            && err.contains("`call_bind`"),
        "malformed callable family must fail at its first semantic violation: {err}"
    );
}

#[test]
fn test_compile_checked_lowers_matmul_dunder_dispatch() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "matmul_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![OpIR {
                kind: "matmul".to_string(),
                out: Some("v0".to_string()),
                args: Some(vec!["v1".to_string(), "v2".to_string()]),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);
    assert!(
        source.contains("function molt_matmul")
            && source.contains("local v0 = molt_matmul(v1, v2)")
            && source.contains("molt_get_attr(a, \"__matmul__\")")
            && source.contains("molt_get_attr(b, \"__rmatmul__\")"),
        "matmul should share Luau descriptor lookup authority, got:\n{source}"
    );
    assert!(
        !source.contains("[unsupported op: matmul]"),
        "matmul must not leave checked-output markers, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_lowers_matmul_not_implemented_reflection() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "matmul_not_implemented_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const_not_implemented".to_string(),
                    out: Some("not_impl".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "matmul".to_string(),
                    out: Some("v0".to_string()),
                    args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);
    assert!(
        source.contains("molt_not_implemented = {__molt_not_implemented = true}")
            && source.contains("local not_impl = molt_not_implemented")
            && source.contains("if result ~= molt_not_implemented then return result end"),
        "matmul should use a concrete NotImplemented sentinel, got:\n{source}"
    );
    assert!(
        !source.contains("[unsupported op: matmul]"),
        "matmul NotImplemented path must not leave checked-output markers, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_lowers_inplace_matmul_dunder_dispatch() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "inplace_matmul_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![OpIR {
                kind: "inplace_matmul".to_string(),
                out: Some("v0".to_string()),
                args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend.compile(&ir);
    assert!(
        source.contains("function molt_inplace_matmul")
            && source.contains("local v0 = molt_inplace_matmul(lhs, rhs)")
            && source.contains("molt_get_attr(a, \"__imatmul__\")")
            && source.contains("return molt_matmul_impl(a, b, \"@=\")"),
        "inplace matmul should try __imatmul__ before binary fallback, got:\n{source}"
    );
    assert!(
        !source.contains("[unsupported op: inplace_matmul]"),
        "inplace matmul must not leave checked-output markers, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_rejects_call_async_scheduler_semantics() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: "call_async_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("payload".to_string()),
                    value: Some(5),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "call_async".to_string(),
                    s_value: Some("poll_target".to_string()),
                    args: Some(vec!["payload".to_string()]),
                    out: Some("awaited".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["awaited".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let error = LuauBackend::new()
        .compile_checked(&ir)
        .expect_err("Luau has no exact Molt task-construction or scheduler model");
    assert!(error.contains("`call_async`") && error.contains("exact async scheduler"));
}

#[test]
fn test_compile_checked_rejects_native_awaitable_without_async_runtime() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "native_awaitable_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![
                OpIR {
                    kind: "object_new".to_string(),
                    out: Some("awaitable".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is_native_awaitable".to_string(),
                    out: Some("is_native".to_string()),
                    args: Some(vec!["awaitable".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let error = LuauBackend::new()
        .compile_checked(&ir)
        .expect_err("native-awaitable identity requires the rejected async runtime family");
    assert!(error.contains("`is_native_awaitable`") && error.contains("exact async scheduler"));
}

#[test]
fn test_compile_checked_rejects_file_marker() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "file_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![OpIR {
                kind: "file_open".to_string(),
                out: Some("v0".to_string()),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let err = backend
        .compile_checked(&ir)
        .expect_err("compile_checked must reject unsupported file operations");
    assert!(
        err.contains("rejected before source generation")
            && err.contains("`file_open`")
            && err.contains("host filesystem"),
        "error should come from generated pre-source admission, got: {err}"
    );
}

#[test]
fn test_compile_checked_rejects_context_marker() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "context_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            parameter_custody: Vec::new(),
            execution_context: ExecutionContextPolicy::None,
            ops: vec![OpIR {
                kind: "context_enter".to_string(),
                out: Some("v0".to_string()),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let err = backend
        .compile_checked(&ir)
        .expect_err("compile_checked must reject unsupported context operations");
    assert!(
        err.contains("rejected before source generation")
            && err.contains("`context_enter`")
            && err.contains("unclassified"),
        "error should come from generated pre-source admission, got: {err}"
    );
}
