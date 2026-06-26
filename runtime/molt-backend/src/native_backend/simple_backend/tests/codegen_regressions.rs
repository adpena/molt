use super::*;

#[test]
fn annotate_function_object_compiles_without_signature_mismatch() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "_sitebuiltins____annotate__".to_string(),
                params: vec!["format".to_string()],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "func_new".to_string(),
                        s_value: Some("_sitebuiltins____annotate__".to_string()),
                        value: Some(1),
                        out: Some("annotate_fn".to_string()),
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
            },
        ],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
}

#[test]
fn guarded_void_function_object_compiles_without_result_panic() {
    let ir = SimpleIR {
        functions: vec![
            FunctionIR {
                name: "void_helper".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
            FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "func_new".to_string(),
                        s_value: Some("void_helper".to_string()),
                        value: Some(0),
                        out: Some("void_fn".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "call_guarded".to_string(),
                        s_value: Some("void_helper".to_string()),
                        args: Some(vec!["void_fn".to_string()]),
                        out: Some("result".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        var: Some("result".to_string()),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
            },
        ],
        profile: None,
    };

    let output = SimpleBackend::new().compile(ir);

    assert!(!output.bytes.is_empty());
}

#[test]
fn direct_imported_runtime_call_avoids_guarded_call_wrapper() {
    let func = FunctionIR {
        name: "hot_runtime_call".to_string(),
        params: vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
            "f".to_string(),
            "g".to_string(),
            "h".to_string(),
        ],
        ops: vec![
            OpIR {
                kind: "call".to_string(),
                s_value: Some("molt_gpu_linear_contiguous".to_string()),
                args: Some(vec![
                    "a".to_string(),
                    "b".to_string(),
                    "c".to_string(),
                    "d".to_string(),
                    "e".to_string(),
                    "f".to_string(),
                    "g".to_string(),
                    "h".to_string(),
                ]),
                out: Some("out".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("out".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let clif = compile_function_to_clif_text(vec![func], "hot_runtime_call");

    assert!(
        !clif.contains("molt_guarded_call"),
        "direct imported runtime calls should not route through molt_guarded_call:\n{clif}"
    );
    assert!(
        !clif.contains("explicit_slot"),
        "direct imported runtime calls should not spill args for the guarded-call wrapper:\n{clif}"
    );
}

#[test]
fn native_boxed_or_retains_selected_operand_result() {
    let func = FunctionIR {
        name: "boxed_or_selected_owner".to_string(),
        params: vec!["lhs".to_string(), "rhs".to_string()],
        ops: vec![
            OpIR {
                kind: "or".to_string(),
                args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                out: Some("selected".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("selected".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let clif = compile_function_to_clif_text(vec![func], "boxed_or_selected_owner");
    let selected = clif
        .lines()
        .find_map(|line| line.trim().split_once(" = select ").map(|(v, _)| v.trim()))
        .unwrap_or_else(|| panic!("boxed or must emit a selected result:\n{clif}"));
    let selected_call = format!("({selected})");
    assert!(
        clif.lines().any(|line| {
            let line = line.trim();
            line.starts_with("call fn") && line.contains(&selected_call)
        }),
        "boxed or must retain the selected result before returning it:\n{clif}"
    );
}

#[test]
fn native_shift_lowering_uses_runtime_without_shift_count_proof() {
    let func = FunctionIR {
        name: "shift_runtime_contract".to_string(),
        params: vec!["lhs".to_string(), "rhs".to_string()],
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                value: Some(8),
                out: Some("const_lhs".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "const".to_string(),
                value: Some(1),
                out: Some("const_rhs".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "lshift".to_string(),
                args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                out: Some("left".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "rshift".to_string(),
                args: Some(vec!["lhs".to_string(), "rhs".to_string()]),
                out: Some("right".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "shl".to_string(),
                args: Some(vec!["const_lhs".to_string(), "const_rhs".to_string()]),
                out: Some("const_left".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "shr".to_string(),
                args: Some(vec!["const_lhs".to_string(), "const_rhs".to_string()]),
                out: Some("const_right".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["left".to_string(), "right".to_string()]),
                out: Some("param_sum".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["const_left".to_string(), "const_right".to_string()]),
                out: Some("const_sum".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["param_sum".to_string(), "const_sum".to_string()]),
                out: Some("out".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                var: Some("out".to_string()),
                ..OpIR::default()
            },
        ],
        param_types: Some(vec!["int".to_string(), "int".to_string()]),
        source_file: None,
        is_extern: false,
    };

    let clif = compile_function_to_clif_text(vec![func], "shift_runtime_contract");
    let binary_calls = clif
        .lines()
        .filter(|line| line.contains(" = call fn"))
        .count();

    assert!(
        binary_calls >= 4,
        "native shifts over typed params and raw-primary constants must remain runtime calls until range and shift-count proof exists:\n{clif}"
    );
    assert!(
        !clif.contains("ishl.i64 v1, v2") && !clif.contains("sshr v1, v2"),
        "native shifts must not lower directly to raw Cranelift shifts without proof:\n{clif}"
    );
}

#[test]
fn nested_exception_raise_if_does_not_synthesize_zero_predecessors() {
    let clif = compile_function_to_clif_text(
        vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const_bool".to_string(),
                    value: Some(0),
                    out: Some("flag".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "if".to_string(),
                    args: Some(vec!["flag".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "else".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_last".to_string(),
                    out: Some("exc".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_none".to_string(),
                    out: Some("nonev".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "is".to_string(),
                    args: Some(vec!["exc".to_string(), "nonev".to_string()]),
                    out: Some("is_none".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "not".to_string(),
                    args: Some(vec!["is_none".to_string()]),
                    out: Some("has_exc".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "if".to_string(),
                    args: Some(vec!["has_exc".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "exception_stack_clear".to_string(),
                    out: Some("none".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "raise".to_string(),
                    args: Some(vec!["exc".to_string()]),
                    out: Some("none".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "jump".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "end_if".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "end_if".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "label".to_string(),
                    value: Some(1),
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
        }],
        "molt_main",
    );

    let suspicious: Vec<&str> = clif
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("jump block") && line.contains(" = 0"))
        .collect();

    assert!(
        suspicious.is_empty(),
        "nested exception raise CFG synthesized zero-valued predecessors:\n{}\n\nCLIF:\n{}",
        suspicious.join("\n"),
        clif
    );
}

#[test]
fn fast_int_overflow_result_does_not_unbox_merged_bigint_result() {
    let clif = compile_function_to_clif_text(
        vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("base".to_string()),
                    value: Some(2),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("exp".to_string()),
                    value: Some(63),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "pow".to_string(),
                    args: Some(vec!["base".to_string(), "exp".to_string()]),
                    out: Some("powv".to_string()),
                    fast_int: Some(true),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("one".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "sub".to_string(),
                    args: Some(vec!["powv".to_string(), "one".to_string()]),
                    out: Some("maxsize".to_string()),
                    fast_int: Some(true),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("maxsize".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        "molt_main",
    );

    assert!(
        !clif.contains("block11(v43: i64):\n    v77 = iconst.i64 0x7fff_0000_0000_0000"),
        "merged overflow result must remain boxed until a real inline-int consumer proves otherwise:\n{clif}",
    );
}

#[test]
fn bool_primary_loop_compare_does_not_materialize_boxed_bool() {
    let clif = compile_function_to_clif_text(
        vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![
                OpIR {
                    kind: "const".to_string(),
                    out: Some("init".to_string()),
                    value: Some(0),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("one".to_string()),
                    value: Some(1),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const".to_string(),
                    out: Some("limit".to_string()),
                    value: Some(10),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("_bb1_arg0".to_string()),
                    args: Some(vec!["init".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_start".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "load_var".to_string(),
                    out: Some("i_cur".to_string()),
                    var: Some("_bb1_arg0".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "lt".to_string(),
                    out: Some("keep_going".to_string()),
                    args: Some(vec!["i_cur".to_string(), "limit".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_break_if_false".to_string(),
                    args: Some(vec!["keep_going".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "add".to_string(),
                    out: Some("i_next".to_string()),
                    args: Some(vec!["i_cur".to_string(), "one".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "store_var".to_string(),
                    var: Some("_bb1_arg0".to_string()),
                    args: Some(vec!["i_next".to_string()]),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_continue".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "loop_end".to_string(),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "ret".to_string(),
                    var: Some("i_cur".to_string()),
                    ..OpIR::default()
                },
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        }],
        "molt_main",
    );

    assert!(
        clif.contains("icmp slt"),
        "loop comparison should lower to a raw signed compare:\n{clif}"
    );
    assert!(
        !clif.contains("0x7ffa_0000_0000_0000"),
        "bool-primary loop compare should not materialize a NaN-boxed bool:\n{clif}"
    );
}
