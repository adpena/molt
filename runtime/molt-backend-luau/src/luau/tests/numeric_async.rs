use super::*;

#[test]
fn test_compile_checked_lowers_checked_add_helper() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "checked_add_test".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
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
    let source = backend
        .compile_checked(&ir)
        .expect("checked_add should lower without stub markers");

    assert!(source.contains("local function molt_checked_i64_add"));
    assert!(source.contains("return a + b, false"));
    assert!(source.contains("local sum: number, overflow: boolean = molt_checked_i64_add(a, b)"));
    assert!(!source.contains("[unsupported op: checked_add]"));
}

#[test]
fn test_compile_checked_lowers_checked_mul_helper() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "checked_mul_test".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
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
    let source = backend
        .compile_checked(&ir)
        .expect("checked_mul should lower without stub markers");

    assert!(source.contains("local function molt_checked_i64_mul"));
    assert!(source.contains("if p >= 9007199254740992 or p <= -9007199254740992"));
    assert!(
        source.contains("local product: number, overflow: boolean = molt_checked_i64_mul(a, b)")
    );
    assert!(!source.contains("[unsupported op: checked_mul]"));
}

#[test]
fn test_compile_checked_lowers_zero_division_guards() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "zero_division_guard_test".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
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
    let source = backend
        .compile_checked(&ir)
        .expect("division ops should lower with Python zero-division guards");

    assert!(source.contains("__msg=\"division by zero\""));
    assert!(source.contains("__msg=\"integer modulo by zero\""));
    assert!(source.contains("__msg=\"integer division or modulo by zero\""));
    assert!(source.contains("local quotient: number = a / b"));
    assert!(source.contains("local remainder: number = a % b"));
    assert!(source.contains("local floor_quotient: number = a // b"));
    assert!(!source.contains("[unsupported op: div]"));
    assert!(!source.contains("[unsupported op: mod]"));
    assert!(!source.contains("[unsupported op: floordiv]"));
}

#[test]
fn test_compile_checked_lowers_pow_mod_square_multiply_loop() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "pow_mod_test".to_string(),
            params: vec!["base".to_string(), "exp".to_string(), "modulus".to_string()],
            param_types: Some(vec![
                "int".to_string(),
                "int".to_string(),
                "int".to_string(),
            ]),
            source_file: None,
            is_extern: false,
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
    let source = backend
        .compile_checked(&ir)
        .expect("pow_mod should lower without stub markers");

    assert!(source.contains("local result; do local __b, __e, __m = base % modulus, exp, modulus"));
    assert!(source.contains("while __e > 0 do"));
    assert!(source.contains("__r = (__r * __b) % __m"));
    assert!(source.contains("__e = __e // 2"));
    assert!(!source.contains("[unsupported op: pow_mod]"));
}

#[test]
fn test_compile_checked_lowers_vector_reduction_kernels() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "vector_sum_kernel_test".to_string(),
                params: vec!["values".to_string()],
                param_types: Some(vec!["list".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "vec_sum_i64".to_string(),
                        args: Some(vec!["values".to_string()]),
                        out: Some("sum_result".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["sum_result".to_string()]),
                        ..OpIR::default()
                    },
                ],
            },
            FunctionIR {
                name: "vector_min_kernel_test".to_string(),
                params: vec!["values".to_string()],
                param_types: Some(vec!["list".to_string()]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "vec_min_i64".to_string(),
                        args: Some(vec!["values".to_string()]),
                        out: Some("min_result".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["min_result".to_string()]),
                        ..OpIR::default()
                    },
                ],
            },
        ],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("vector reductions should lower without stub markers");

    assert!(source.contains("local sum_result\n\tdo"));
    assert!(source.contains("local acc = 0"));
    assert!(source.contains("for __vi = 1, #values do local v = values[__vi]; acc = acc + v end"));
    assert!(source.contains("local min_result\n\tdo"));
    assert!(source.contains("local acc = math.huge"));
    assert!(source.contains(
        "for __vi = 1, #values do local v = values[__vi]; if v < acc then acc = v end end"
    ));
    assert!(!source.contains("[unsupported op: vec_sum_i64]"));
    assert!(!source.contains("[unsupported op: vec_min_i64]"));
}

#[test]
fn test_compile_checked_lowers_intarray_from_seq_dense_integer_table() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "intarray_from_seq_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("one".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("two".to_string()),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "list_new".to_string(),
                    out: Some("seq".to_string()),
                    args: Some(vec!["one".to_string(), "two".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "intarray_from_seq".to_string(),
                    out: Some("arr".to_string()),
                    args: Some(vec!["seq".to_string()]),
                    ..OpIR::default()
                },
            ],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("intarray_from_seq should lower to a dense Luau integer table");
    assert!(
        source.contains("local arr\n")
            && source.contains("\tdo\n")
            && source.contains("local __seq = seq")
            && source.contains("local __arr = {}")
            && source.contains("math.floor(__v) == __v")
            && source.contains("arr = if __ok then __arr else nil")
            && source.contains("arr = nil"),
        "intarray_from_seq should copy integer tables and fail closed, got:\n{source}"
    );
    assert!(
        !source.contains("[intarray_from_seq]")
            && !source.contains("[unsupported op: intarray_from_seq]"),
        "intarray_from_seq must not leave checked-output markers, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_lowers_fused_dict_kernels() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "split_ws_dict_inc_test".to_string(),
                params: vec!["line".to_string(), "dict".to_string(), "delta".to_string()],
                param_types: Some(vec![
                    "str".to_string(),
                    "dict".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "string_split_ws_dict_inc".to_string(),
                        args: Some(vec![
                            "line".to_string(),
                            "dict".to_string(),
                            "delta".to_string(),
                        ]),
                        out: Some("ws_result".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["ws_result".to_string()]),
                        ..OpIR::default()
                    },
                ],
            },
            FunctionIR {
                name: "split_sep_dict_inc_test".to_string(),
                params: vec![
                    "line".to_string(),
                    "sep".to_string(),
                    "dict".to_string(),
                    "delta".to_string(),
                ],
                param_types: Some(vec![
                    "str".to_string(),
                    "str".to_string(),
                    "dict".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "string_split_sep_dict_inc".to_string(),
                        args: Some(vec![
                            "line".to_string(),
                            "sep".to_string(),
                            "dict".to_string(),
                            "delta".to_string(),
                        ]),
                        out: Some("sep_result".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["sep_result".to_string()]),
                        ..OpIR::default()
                    },
                ],
            },
            FunctionIR {
                name: "taq_ingest_line_test".to_string(),
                params: vec![
                    "dict".to_string(),
                    "line".to_string(),
                    "bucket_size".to_string(),
                ],
                param_types: Some(vec![
                    "dict".to_string(),
                    "str".to_string(),
                    "int".to_string(),
                ]),
                source_file: None,
                is_extern: false,
                ops: vec![
                    OpIR {
                        kind: "taq_ingest_line".to_string(),
                        args: Some(vec![
                            "dict".to_string(),
                            "line".to_string(),
                            "bucket_size".to_string(),
                        ]),
                        out: Some("ingested".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["ingested".to_string()]),
                        ..OpIR::default()
                    },
                ],
            },
        ],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("fused dict kernels should lower without unsupported markers");

    assert!(source.contains("local function molt_string_split_ws_dict_inc"));
    assert!(source.contains("local function molt_string_split_sep_dict_inc"));
    assert!(source.contains("local function molt_taq_ingest_line"));
    assert!(!source.contains("local molt_string = {"));
    assert!(source.contains("local ws_result = molt_string_split_ws_dict_inc(line, dict, delta)"));
    assert!(
        source
            .contains("local sep_result = molt_string_split_sep_dict_inc(line, sep, dict, delta)")
    );
    assert!(source.contains("local ingested = molt_taq_ingest_line(dict, line, bucket_size)"));
    assert!(
        source.contains(
            "series[#series + 1] = {molt_taq_div_euclid(timestamp, bucket_size), volume}"
        )
    );
    assert!(!source.contains("[unsupported op: string_split_ws_dict_inc]"));
    assert!(!source.contains("[unsupported op: string_split_sep_dict_inc]"));
    assert!(!source.contains("[unsupported op: taq_ingest_line]"));
}

#[test]
fn test_compile_checked_lowers_labeled_branch_ops() {
    let branch_function = |name: &str, kind: &str, label: i64, flag_value: i64| FunctionIR {
        name: name.to_string(),
        params: Vec::new(),
        param_types: None,
        source_file: None,
        is_extern: false,
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
    let source = backend
        .compile_checked(&ir)
        .expect("labeled branch ops should lower without unsupported markers");

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
            name: "unsupported_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![OpIR {
                kind: "unknown_luau_op".to_string(),
                out: Some("v0".to_string()),
                args: Some(vec!["v1".to_string(), "v2".to_string()]),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let err = backend
        .compile_via_ir(&ir)
        .expect_err("preview/IR path must reject unsupported output");
    assert!(err.contains("unsupported marker"));
    assert!(err.contains("[unsupported op: unknown_luau_op]"));
}

#[test]
fn test_compile_checked_lowers_matmul_dunder_dispatch() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "matmul_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
    let source = backend
        .compile_checked(&ir)
        .expect("matmul should lower through Luau dunder helper");
    assert!(
        source.contains("local function molt_matmul")
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
            name: "matmul_not_implemented_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
    let source = backend
        .compile_checked(&ir)
        .expect("NotImplemented-aware matmul should lower without markers");
    assert!(
        source.contains("local molt_not_implemented = {__molt_not_implemented = true}")
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
            name: "inplace_matmul_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
    let source = backend
        .compile_checked(&ir)
        .expect("inplace matmul should lower through Luau dunder helper");
    assert!(
        source.contains("local function molt_inplace_matmul")
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
fn test_compile_checked_rejects_async_marker() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "async_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
            ops: vec![OpIR {
                kind: "spawn".to_string(),
                out: Some("v0".to_string()),
                ..OpIR::default()
            }],
        }],
        profile: None,
    };
    let mut backend = LuauBackend::new();
    let err = backend
        .compile_checked(&ir)
        .expect_err("compile_checked must reject async stub markers");
    assert!(
        err.contains("semantic stub marker"),
        "error should mention semantic stub marker, got: {err}"
    );
}

#[test]
fn test_compile_checked_lowers_call_async_poll_target_directly() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "call_async_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("call_async with a known poll target should lower directly");

    assert!(
        source.contains("local awaited = poll_target(payload)"),
        "call_async should invoke the s_value poll target directly, got:\n{source}"
    );
    assert!(
        !source.contains("[async: call_async]") && !source.contains("[unsupported op: call_async]"),
        "call_async must not leave async stub markers, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_lowers_is_native_awaitable_target_fact() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "native_awaitable_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
    let mut backend = LuauBackend::new();
    let source = backend
        .compile_checked(&ir)
        .expect("is_native_awaitable should lower as a Luau target fact");
    assert!(
        source.contains("local is_native = false"),
        "Luau has no native Molt poll-function objects, got:\n{source}"
    );
    assert!(
        !source.contains("[async: is_native_awaitable]")
            && !source.contains("[unsupported op: is_native_awaitable]"),
        "is_native_awaitable must not lower through async stubs, got:\n{source}"
    );
}

#[test]
fn test_compile_checked_rejects_file_marker() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "file_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
        .expect_err("compile_checked must reject file stub markers");
    assert!(
        err.contains("semantic stub marker"),
        "error should mention semantic stub marker, got: {err}"
    );
}

#[test]
fn test_compile_checked_rejects_context_marker() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "context_test".to_string(),
            params: vec![],
            param_types: None,
            source_file: None,
            is_extern: false,
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
        .expect_err("compile_checked must reject context stub markers");
    assert!(
        err.contains("semantic stub marker"),
        "error should mention semantic stub marker, got: {err}"
    );
}
