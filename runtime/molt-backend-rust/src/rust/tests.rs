use super::*;
use crate::{FunctionIR, OpIR, SimpleIR};

fn compile_and_run_emitted(source: &str, stem: &str) -> String {
    let temp = std::env::temp_dir().join(format!("molt_rust_{stem}_{}", std::process::id()));
    std::fs::create_dir_all(&temp).expect("create emitted-Rust test directory");
    let source_path = temp.join("main.rs");
    let binary_path = temp.join(if cfg!(windows) { "main.exe" } else { "main" });
    std::fs::write(&source_path, source).expect("write emitted Rust source");
    let compile = std::process::Command::new("rustc")
        .args(["--edition", "2024", "-A", "warnings", "-O", "-o"])
        .arg(&binary_path)
        .arg(&source_path)
        .status()
        .expect("rustc must compile emitted Rust backend source");
    assert!(compile.success(), "emitted Rust source must compile");
    let output = std::process::Command::new(&binary_path)
        .output()
        .expect("emitted Rust binary must execute");
    assert!(output.status.success(), "emitted Rust binary must succeed");
    let stdout = String::from_utf8(output.stdout).expect("emitted stdout must be UTF-8");
    let _ = std::fs::remove_dir_all(temp);
    stdout
}

#[test]
fn emitted_stack_clear_preserves_the_nested_execution_baseline() {
    let op = |kind: &str| OpIR {
        kind: kind.to_string(),
        ..OpIR::default()
    };
    let mut caller_depth = op("const_bool");
    caller_depth.out = Some("caller_depth".to_string());
    caller_depth.value = Some(1);
    let mut set_caller_depth = op("exception_stack_set_depth");
    set_caller_depth.args = Some(vec!["caller_depth".to_string()]);
    let mut enter = op("exception_stack_enter");
    enter.out = Some("previous_baseline".to_string());
    let mut observed = op("exception_stack_depth");
    observed.out = Some("observed_depth".to_string());
    let mut print = op("print");
    print.args = Some(vec!["observed_depth".to_string()]);
    let mut exit = op("exception_stack_exit");
    exit.args = Some(vec!["previous_baseline".to_string()]);
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                caller_depth,
                set_caller_depth,
                enter,
                op("exception_stack_clear"),
                observed,
                print,
                exit,
                op("ret_void"),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };
    // Production correctly rejects exception-state programs until the Rust
    // target grows a complete exception runtime. This unit exercises the
    // standalone prelude that preview/source emission already maintains.
    let source = RustBackend::new().compile(&ir);
    assert!(source.contains("*depth.borrow_mut() = baseline"));
    assert!(!source.contains("*baseline.borrow_mut() = 0"));

    assert_eq!(
        compile_and_run_emitted(&source, "exception_stack_baseline").trim(),
        "1"
    );
}

#[test]
fn compile_checked_keeps_ordinary_programs_available() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "ret_void".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let source = RustBackend::new()
        .compile_checked(&ir)
        .expect("ordinary Rust programs must not require the native pending-call boundary");
    assert!(source.contains("fn molt_main"));
}

#[test]
fn compile_checked_rejects_async_work_poll_without_runtime_boundary() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "async_work_poll_test".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "async_work_poll".to_string(),
                value: Some(0),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let err = backend
        .compile_checked(&ir)
        .expect_err("Rust must not erase the pending-call/eval-breaker poll");
    assert!(
        err.contains("async_work_poll"),
        "diagnostic must name the op: {err}"
    );
    assert!(
        err.contains("canonical pending-call/eval-breaker runtime boundary is unavailable"),
        "diagnostic must name the missing target capability: {err}"
    );
}

#[test]
fn compile_keeps_annotation_functions_when_referenced() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "__main____annotate__".to_string(),
                params: vec!["args".to_string()],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("fn __main____annotate__("));
}

#[test]
fn compile_int_from_str_of_obj_records_unsupported_integer_authority() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![
                "value".to_string(),
                "base".to_string(),
                "has_base".to_string(),
            ],
            ops: vec![
                OpIR {
                    kind: "int_from_str_of_obj".to_string(),
                    args: Some(vec![
                        "value".to_string(),
                        "base".to_string(),
                        "has_base".to_string(),
                    ]),
                    out: Some("out".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["out".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(!source.contains("i64::from_str_radix"));
    assert!(
        backend
            .unsupported_ops
            .iter()
            .any(|failure| failure.contains("int_from_str_of_obj"))
    );
}

#[test]
fn compile_numeric_equality_does_not_fall_back_for_non_numeric_values() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "cmp_eq".to_string(),
                args: Some(vec!["v0".to_string(), "v1".to_string()]),
                out: Some("v2".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("fn molt_is_numeric(x: &MoltValue) -> bool"));
    assert!(source.contains("_ if molt_is_numeric(a) && molt_is_numeric(b) =>"));
    assert!(source.contains("_ => false,"));
}

#[test]
fn compile_checked_rejects_untyped_integer_capable_arithmetic_before_emission() {
    let mut backend = RustBackend::new();
    let mut add = OpIR {
        kind: "add".to_string(),
        args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
        out: Some("sum".to_string()),
        ..OpIR::default()
    };
    add.fast_int = Some(true);
    add.type_hint = Some("int".to_string());
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "helper".to_string(),
            params: vec!["lhs".to_string(), "rhs".to_string()],
            ops: vec![
                add,
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["sum".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let error = backend
        .compile_checked(&ir)
        .expect_err("transport hints cannot admit integer-capable Rust arithmetic");
    assert!(error.contains("rejected before source generation"));
    assert!(error.contains("lacks arbitrary-precision integers"));
    assert!(backend.output.is_empty(), "admission must precede emission");
    assert!(
        backend.unsupported_ops.is_empty(),
        "dispatch must not run after admission rejects"
    );
}

#[test]
fn compile_checked_rejects_typed_integer_arithmetic_before_emission() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "helper".to_string(),
            params: vec!["lhs".to_string(), "rhs".to_string()],
            ops: vec![
                OpIR {
                    kind: "add".to_string(),
                    args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                    out: Some("sum".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["sum".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["int".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let error = backend
        .compile_checked(&ir)
        .expect_err("i64 arithmetic is not Python arbitrary-precision arithmetic");
    assert!(error.contains("rejected before source generation"));
    assert!(error.contains("helper:op#0 `add`"));
    assert!(backend.output.is_empty(), "admission must precede emission");
}

#[test]
fn compile_list_append_writes_back_indexed_aliases() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "helper".to_string(),
                params: vec!["v0".to_string(), "v1".to_string(), "v3".to_string()],
                ops: vec![
                    OpIR {
                        kind: "index".to_string(),
                        args: Some(vec!["v0".to_string(), "v1".to_string()]),
                        out: Some("v2".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "list_append".to_string(),
                        args: Some(vec!["v2".to_string(), "v3".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("let mut __alias_key_v2: MoltValue = v1.clone();"));
    assert!(source.contains("molt_list_append(&mut v2, v3.clone());"));
    assert!(source.contains("molt_set_item(&mut v0, __alias_key_v2.clone(), v2.clone());"));
}

#[test]
fn compile_call_method_uses_s_value_method_name() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec!["items".to_string(), "value".to_string()],
            ops: vec![
                OpIR {
                    kind: "call_method".to_string(),
                    s_value: Some("append".to_string()),
                    args: Some(vec!["items".to_string(), "value".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["list[int]".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("molt_list_append(&mut items, value.clone());"));
    assert!(!source.contains("MOLT_STUB: method"));
}

#[test]
fn compile_ord_at_emits_fused_helper() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "ord_at_unicode".to_string(),
            params: vec!["s".to_string(), "i".to_string()],
            ops: vec![
                OpIR {
                    kind: "ord_at".to_string(),
                    args: Some(vec!["s".to_string(), "i".to_string()]),
                    out: Some("code".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["code".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: Some(vec!["str".to_string(), "int".to_string()]),
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("fn molt_ord_at(obj: &MoltValue, key: &MoltValue)"));
    assert!(source.contains("fn molt_get_item(obj: &MoltValue, key: &MoltValue)"));
    assert!(source.contains("let normalized = if idx < 0 { len as i64 + idx } else { idx };"));
    assert!(source.contains("normalized < 0 || normalized >= len as i64"));
    assert!(source.contains("usize::try_from(idx).ok().and_then(|i| s.chars().nth(i))"));
    assert!(!source.contains("chars: Vec<char>"));
    assert!(!source.contains(".max(0) as usize"));
    assert!(source.contains("panic!(\"KeyError: {}\", molt_repr_inner(key))"));
    assert!(source.contains("fn molt_ord(x: &MoltValue)"));
    assert!(source.contains("let mut code: MoltValue = molt_ord_at(&s, &i);"));
    assert!(!source.contains("MOLT_STUB"));
}

#[test]
fn compile_checked_rejects_code_slots_exception_and_refcount_models() {
    let cases = [
        (
            "code_slots",
            Vec::new(),
            OpIR {
                kind: "code_slots_init".to_string(),
                value: Some(4),
                ..OpIR::default()
            },
            "operation requires Python aliasing, cycles, None storage, hashing, and object protocols",
        ),
        (
            "exception_state",
            Vec::new(),
            OpIR {
                kind: "exception_clear".to_string(),
                ..OpIR::default()
            },
            "operation requires Python exception state, matching, and structured unwinding",
        ),
        (
            "deterministic_lifetime",
            vec!["value".to_string()],
            OpIR {
                kind: "inc_ref".to_string(),
                args: Some(vec!["value".to_string()]),
                ..OpIR::default()
            },
            "operation requires deterministic Python lifetime/finalizer semantics",
        ),
    ];

    for (name, params, unsupported_op, expected_reason) in cases {
        let expected_kind = unsupported_op.kind.clone();
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: name.to_string(),
                params,
                ops: vec![
                    unsupported_op,
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
                execution_context: Default::default(),
            }],
            profile: None,
        };
        let mut backend = RustBackend::new();
        let error = backend
            .compile_checked(&ir)
            .expect_err("Rust target must reject each unsupported runtime authority");
        assert!(
            error.contains("rejected before source generation")
                && error.contains(&expected_kind)
                && error.contains(expected_reason),
            "{name} must reach its own generated admission reason: {error}"
        );
        assert!(backend.output.is_empty(), "{name} emitted partial source");
    }
}

#[test]
fn compile_checked_rejects_unsupported_dispatch() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "unsupported".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "matmul".to_string(),
                out: Some("value".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let err = backend
        .compile_checked(&ir)
        .expect_err("unsupported ops must be rejected at the Result boundary");
    assert!(
        err.contains("operation is unclassified in the generated runtime semantic authority"),
        "error must come from generated pre-source admission, got: {err}"
    );
    assert!(
        err.contains("matmul"),
        "diagnostic must name the op kind, got: {err}"
    );
}

#[test]
fn compile_boolean_short_circuit_omits_unused_if_parentheses() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "and".to_string(),
                    args: Some(vec!["v0".to_string(), "v1".to_string()]),
                    out: Some("v2".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "or".to_string(),
                    args: Some(vec!["v0".to_string(), "v1".to_string()]),
                    out: Some("v3".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("if !molt_bool(&v0) { v0.clone() } else { v1.clone() }"));
    assert!(source.contains("if molt_bool(&v0) { v0.clone() } else { v1.clone() }"));
    assert!(!source.contains("(if !molt_bool(&v0) { v0.clone() } else { v1.clone() })"));
    assert!(!source.contains("(if molt_bool(&v0) { v0.clone() } else { v1.clone() })"));
}

#[test]
fn compile_unpack_sequence_uses_exact_arity_runtime_authority() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    value: Some(10),
                    out: Some("old_item".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "build_list".to_string(),
                    args: Some(vec!["old_item".to_string()]),
                    out: Some("old".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".to_string(),
                    var: Some("old".to_string()),
                    out: Some("left".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(20),
                    out: Some("new_item".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "build_list".to_string(),
                    args: Some(vec!["new_item".to_string()]),
                    out: Some("new_value".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(7),
                    out: Some("right_value".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "build_list".to_string(),
                    args: Some(vec!["new_value".to_string(), "right_value".to_string()]),
                    out: Some("seq".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "unpack_sequence".to_string(),
                    args: Some(vec![
                        "seq".to_string(),
                        "left".to_string(),
                        "right".to_string(),
                    ]),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(30),
                    out: Some("appended".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "list_append".to_string(),
                    args: Some(vec!["left".to_string(), "appended".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "len".to_string(),
                    args: Some(vec!["left".to_string()]),
                    out: Some("left_len".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "len".to_string(),
                    args: Some(vec!["old".to_string()]),
                    out: Some("old_len".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "print".to_string(),
                    args: Some(vec![
                        "left_len".to_string(),
                        "old_len".to_string(),
                        "right".to_string(),
                    ]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("fn molt_unpack_sequence("));
    assert!(source.contains("molt_unpack_sequence(&seq, 2).into_iter()"));
    let list_arm = source
        .find("MoltValue::List(values) =>")
        .expect("list unpack arm");
    let list_len = source[list_arm..]
        .find("let actual = values.len();")
        .expect("O(1) list length guard");
    let list_clone = source[list_arm..]
        .find("values.clone()")
        .expect("exact-match list clone");
    assert!(
        list_len < list_clone,
        "list cardinality must be checked before cloning"
    );
    let dict_arm = source
        .find("MoltValue::Dict(entries) =>")
        .expect("dict unpack arm");
    let dict_len = source[dict_arm..]
        .find("let actual = entries.len();")
        .expect("O(1) dict length guard");
    let dict_collect = source[dict_arm..]
        .find("entries.iter().map")
        .expect("exact-match dict key clone");
    assert!(
        dict_len < dict_collect,
        "dict cardinality must be checked before cloning"
    );
    assert!(source.contains("let probe_limit = expected_count.saturating_add(1);"));
    assert!(source.contains("Vec::with_capacity(expected_count.min(value.len()))"));
    assert!(source.contains("while items.len() < probe_limit"));
    assert!(source.contains("let mut left: MoltValue = MoltValue::None;"));
    assert!(source.contains("let mut right: MoltValue = MoltValue::None;"));
    assert!(source.contains("left = __molt_unpack_values.next()"));
    assert!(source.contains("right = __molt_unpack_values.next()"));
    assert_eq!(
        compile_and_run_emitted(&source, "unpack_multi_definition").trim(),
        "2 1 7",
        "unpack outputs must remain in scope and must sever stale aliases"
    );
    assert!(
        !backend
            .unsupported_ops
            .iter()
            .any(|failure| failure.contains("unpack_sequence"))
    );
}

#[test]
fn compile_unpack_sequence_iterates_unicode_scalars_not_utf8_bytes() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("a🦀".to_string()),
                    out: Some("seq".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "unpack_sequence".to_string(),
                    args: Some(vec![
                        "seq".to_string(),
                        "left".to_string(),
                        "right".to_string(),
                    ]),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "print".to_string(),
                    args: Some(vec!["left".to_string(), "right".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };
    let source = backend.compile(&ir);
    assert_eq!(
        compile_and_run_emitted(&source, "unpack_unicode_scalars").trim(),
        "a 🦀"
    );
}

#[test]
fn malformed_simple_ir_unpack_is_reported_not_emitted() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec!["seq".to_string()],
            ops: vec![
                OpIR {
                    kind: "unpack_sequence".to_string(),
                    args: Some(vec!["seq".to_string(), "left".to_string()]),
                    value: None,
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };
    let source = backend.compile(&ir);
    assert!(!source.contains("molt_unpack_sequence(&seq"));
    assert!(
        backend
            .unsupported_ops
            .iter()
            .any(|failure| failure.contains("unpack_sequence"))
    );
}

#[test]
fn compile_module_cache_ops_lower_to_runtime_cache() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("alpha".to_string()),
                    out: Some("name".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_get".to_string(),
                    args: Some(vec!["name".to_string()]),
                    out: Some("miss".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_new".to_string(),
                    args: Some(vec!["name".to_string()]),
                    out: Some("module".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_set".to_string(),
                    args: Some(vec!["name".to_string(), "module".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_get".to_string(),
                    args: Some(vec!["name".to_string()]),
                    out: Some("hit".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_cache_del".to_string(),
                    args: Some(vec!["name".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("fn molt_module_cache_get("));
    assert!(source.contains("fn molt_module_cache_set("));
    assert!(source.contains("fn molt_module_cache_del("));
    assert!(source.contains("let mut miss: MoltValue = molt_module_cache_get(&name);"));
    assert!(source.contains("molt_module_cache_set(&name, module.clone());"));
    assert!(source.contains("let mut hit: MoltValue = molt_module_cache_get(&name);"));
    assert!(source.contains("molt_module_cache_del(&name);"));
    assert!(!source.contains("let mut miss: MoltValue = MoltValue::Bool(true);"));
    assert!(!source.contains("let mut hit: MoltValue = MoltValue::Bool(true);"));
    assert!(!source.contains("MOLT_STUB: module_cache"));
}

#[test]
fn compile_checked_rejects_even_i64_sized_bigint_literals() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_bigint".to_string(),
                    s_value: Some("2305843009213693951".to_string()),
                    out: Some("big".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let error = backend
        .compile_checked(&ir)
        .expect_err("value magnitude cannot shrink Python's bigint semantic domain");
    assert!(error.contains("const_bigint"));
    assert!(error.contains("canonical arbitrary-precision value authority"));
    assert!(backend.output.is_empty());
}

#[test]
fn compile_checked_rejects_unrepresented_literal_values() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_bigint".to_string(),
                    s_value: Some("9223372036854775808".to_string()),
                    out: Some("too_big".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_bytes".to_string(),
                    s_value: Some("payload".to_string()),
                    out: Some("bytes".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_ellipsis".to_string(),
                    out: Some("ellipsis".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let err = backend
        .compile_checked(&ir)
        .expect_err("unsupported literal value representations must fail closed");
    assert!(
        err.contains("rejected before source generation"),
        "got: {err}"
    );
    assert!(err.contains("const_bigint"), "got: {err}");
    assert!(err.contains("canonical arbitrary-precision value authority"));
}

#[test]
fn compile_store_var_and_load_var_use_named_local_storage() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "helper".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("src".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("rows".to_string()),
                    args: Some(vec!["src".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".to_string(),
                    var: Some("rows".to_string()),
                    out: Some("tmp".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    args: Some(vec!["tmp".to_string()]),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let source = backend.compile(&ir);
    assert!(source.contains("let mut rows: MoltValue = MoltValue::None;"));
    assert!(source.contains("rows = src.clone();"));
    assert!(source.contains("let mut tmp: MoltValue = rows.clone();"));
    assert!(!source.contains("MOLT_STUB: store_var"));
    assert!(!source.contains("MOLT_STUB: load_var"));
}

#[test]
fn jump_after_loop_does_not_capture_scoped_set_item_temps() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "helper".to_string(),
            params: vec!["frame".to_string()],
            ops: vec![
                OpIR {
                    kind: "loop_start".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("answer".to_string()),
                    out: Some("key".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    value: Some(42),
                    out: Some("val".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "set_item".to_string(),
                    args: Some(vec![
                        "frame".to_string(),
                        "key".to_string(),
                        "val".to_string(),
                    ]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "jump".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let _source = backend.compile(&ir);
    let err = backend.unsupported_ops.join(", ");
    assert!(err.contains("`jump` (rust backend)"), "got: {err}");
    assert!(err.contains("helper"), "got: {err}");
}

#[test]
fn strip_dead_after_return_skips_jump_after_nested_return_until_else() {
    let ops = vec![
        OpIR {
            kind: "if".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".to_string(),
            value: Some(1),
            ..OpIR::default()
        },
        OpIR {
            kind: "else".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "const".to_string(),
            out: Some("v0".to_string()),
            value: Some(1),
            ..OpIR::default()
        },
        OpIR {
            kind: "end_if".to_string(),
            ..OpIR::default()
        },
    ];

    let lowered = strip_dead_after_return(&ops);
    let kinds: Vec<&str> = lowered.iter().map(|op| op.kind.as_str()).collect();
    assert_eq!(kinds, vec!["if", "ret_void", "else", "const", "end_if"]);
}

#[test]
fn strip_dead_after_return_skips_top_level_jump_after_return() {
    let ops = vec![
        OpIR {
            kind: "ret_void".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".to_string(),
            value: Some(1),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(1),
            ..OpIR::default()
        },
        OpIR {
            kind: "const".to_string(),
            out: Some("v0".to_string()),
            value: Some(1),
            ..OpIR::default()
        },
    ];

    let lowered = strip_dead_after_return(&ops);
    let kinds: Vec<&str> = lowered.iter().map(|op| op.kind.as_str()).collect();
    assert_eq!(kinds, vec!["ret_void"]);
}

/// An op kind that no dispatch arm claims must fail at the Result boundary.
/// The synthetic kind routes to `emit_op_other` and records a dispatch error.
#[test]
fn compile_checked_fails_closed_on_synthetically_unsupported_op() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "molt_synthetic_unsupported_op_probe".to_string(),
                    out: Some("value".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let err = backend
        .compile_checked(&ir)
        .expect_err("a synthetically-unsupported op must fail the build closed");
    assert!(err.contains("unclassified"), "got: {err}");
    assert!(
        err.contains("molt_synthetic_unsupported_op_probe"),
        "diagnostic must name the unsupported op kind, got: {err}"
    );
}

/// Unsupported sink operations fail at the same Result boundary as operations
/// with outputs; neither path emits a substitute value.
#[test]
fn compile_checked_fails_closed_without_emitted_value_marker() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                // No `out`: dispatch still records the unsupported operation.
                kind: "molt_synthetic_unsupported_sink_probe".to_string(),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let err = backend
        .compile_checked(&ir)
        .expect_err("an unsupported op with no output must still fail closed");
    assert!(err.contains("unclassified"), "got: {err}");
    assert!(
        err.contains("molt_synthetic_unsupported_sink_probe"),
        "got: {err}"
    );
}

#[test]
fn compile_checked_rejects_malformed_callable_family_without_substitute_values() {
    let mut backend = RustBackend::new();
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
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
                    kind: "callargs_push_kw".to_string(),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let err = backend
        .compile_checked(&ir)
        .expect_err("malformed callable IR must fail before source publication");
    assert!(
        err.contains("rejected before source generation")
            && err.contains("`call`")
            && err.contains("structured catchable Python exceptions"),
        "malformed callable family must fail at its first semantic violation: {err}"
    );
}
