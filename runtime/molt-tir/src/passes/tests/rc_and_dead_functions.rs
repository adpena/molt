use super::*;

#[test]
fn constant_integer_facts_require_one_canonical_producer() {
    for kind in ["const", "const_int", "load_const"] {
        let constant = |name: &str| OpIR {
            kind: kind.into(),
            out: Some(name.into()),
            value: Some(7),
            ..OpIR::default()
        };
        let function = FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Void,
            name: "constant_identity".into(),
            params: vec!["parameter".into()],
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
            ops: vec![
                constant("stable"),
                constant("parameter"),
                constant("repeated"),
                constant("repeated"),
                constant("repeated"),
                constant("stored"),
                make_store_var("stored", "stable"),
                constant("deleted"),
                OpIR {
                    kind: "delete_var".into(),
                    var: Some("deleted".into()),
                    ..OpIR::default()
                },
                constant("checked"),
                OpIR {
                    kind: "checked_add".into(),
                    var: Some("checked".into()),
                    out: Some("overflow".into()),
                    args: Some(vec!["stable".into(), "stable".into()]),
                    ..OpIR::default()
                },
                constant("none"),
                constant(""),
                OpIR {
                    kind: "store_index".into(),
                    out: Some("stable".into()), // Metadata, not a producer.
                    args: Some(vec!["items".into(), "index".into(), "stable".into()]),
                    ..OpIR::default()
                },
            ],
        };
        assert_eq!(
            build_const_int_map(&function),
            std::collections::BTreeMap::from([("stable".into(), 7)]),
            "{kind}"
        );
    }
}

#[test]
fn rc_coalescing_eliminates_adjacent_inc_dec_pair() {
    let mut func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "test".to_string(),
        params: vec!["x".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
        ops: vec![
            make_ref_op("inc_ref", "x"),
            make_ref_op("dec_ref", "x"),
            make_op("ret_void"),
        ],
    };

    rc_coalescing(&mut func);

    // Both inc_ref and dec_ref should be eliminated.
    assert_eq!(func.ops.len(), 1);
    assert_eq!(func.ops[0].kind, "ret_void");
}

#[test]
fn rc_coalescing_preserves_pair_across_control_flow() {
    let mut func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "test".to_string(),
        params: vec!["x".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
        ops: vec![
            make_ref_op("inc_ref", "x"),
            make_op("if"),
            make_ref_op("dec_ref", "x"),
            make_op("ret_void"),
        ],
    };

    rc_coalescing(&mut func);

    // The pair should NOT be eliminated because `if` is control flow.
    assert_eq!(func.ops.len(), 4);
}

#[test]
fn rc_coalescing_handles_borrow_release_pair() {
    let mut func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "test".to_string(),
        params: vec!["y".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
        ops: vec![
            make_ref_op("borrow", "y"),
            make_ref_op("release", "y"),
            make_op("ret_void"),
        ],
    };

    rc_coalescing(&mut func);

    assert_eq!(func.ops.len(), 1);
    assert_eq!(func.ops[0].kind, "ret_void");
}

#[test]
fn rc_coalescing_preserves_pair_with_intervening_use() {
    let mut func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "test".to_string(),
        params: vec!["x".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
        ops: vec![
            make_ref_op("inc_ref", "x"),
            // An op that uses x as an argument — breaks the window.
            make_arith("add", &["x", "x"], "y"),
            make_ref_op("dec_ref", "x"),
            make_op("ret_void"),
        ],
    };

    rc_coalescing(&mut func);

    // The pair should NOT be eliminated because of the intervening use.
    assert_eq!(func.ops.len(), 4);
}

#[test]
fn rc_coalescing_eliminates_different_vars_independently() {
    let mut func = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Void,
        name: "test".to_string(),
        params: vec!["a".to_string(), "b".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        execution_context: Default::default(),
        ops: vec![
            make_ref_op("inc_ref", "a"),
            make_ref_op("inc_ref", "b"),
            make_ref_op("dec_ref", "a"),
            make_ref_op("dec_ref", "b"),
            make_op("ret_void"),
        ],
    };

    rc_coalescing(&mut func);

    // inc_ref(a)/dec_ref(a) cannot be eliminated because inc_ref(b) intervenes
    // (it doesn't use 'a' though). Let's check what actually happens.
    // The scan finds inc_ref(a) at 0, then looks at 1 (inc_ref(b) — not a
    // dec_ref of a, and doesn't use a), then at 2 (dec_ref(a) — match!).
    // So indices 0,2 are eliminated. Then inc_ref(b) at 1, looks at 3
    // (dec_ref(b) — match!), indices 1,3 eliminated.
    assert_eq!(func.ops.len(), 1);
    assert_eq!(func.ops[0].kind, "ret_void");
}

#[test]
fn protected_runtime_entrypoint_detection_is_explicit() {
    assert!(is_protected_runtime_entrypoint("molt_main"));
    assert!(is_protected_runtime_entrypoint("molt_host_init"));
    assert!(is_protected_runtime_entrypoint("_start"));
    assert!(is_protected_runtime_entrypoint("molt_isolate_import"));
    assert!(is_protected_runtime_entrypoint("molt_isolate_bootstrap"));
    assert!(!is_protected_runtime_entrypoint("molt_init_math"));
    assert!(!is_protected_runtime_entrypoint("user_entry"));
}

#[test]
fn eliminate_dead_functions_retains_runtime_dispatch_closure() {
    let mut ir = SimpleIR {
        functions: vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "entry".to_string(),
                params: vec![],
                ops: vec![make_op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "molt_isolate_import".to_string(),
                params: vec!["p0".to_string()],
                ops: vec![
                    OpIR {
                        kind: "call".to_string(),
                        s_value: Some("molt_init_math".to_string()),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_init_math".to_string(),
                params: vec![],
                ops: vec![make_op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    eliminate_dead_functions(&mut ir);

    let retained: BTreeSet<&str> = ir.functions.iter().map(|func| func.name.as_str()).collect();
    assert!(retained.contains("molt_isolate_import"));
    assert!(retained.contains("molt_init_math"));
    let dispatch = ir
        .functions
        .iter()
        .find(|func| func.name == "molt_isolate_import")
        .expect("runtime dispatch entrypoint must remain");
    assert!(
        dispatch
            .ops
            .iter()
            .any(|op| op.s_value.as_deref() == Some("molt_init_math")),
        "runtime dispatch body must keep its transitive module-init references",
    );
}

#[test]
fn eliminate_dead_functions_retains_molt_host_init_and_transitive_refs() {
    let mut ir = SimpleIR {
        functions: vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "entry".to_string(),
                params: vec![],
                ops: vec![make_op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Value,
                name: "molt_host_init".to_string(),
                params: vec![],
                ops: vec![
                    OpIR {
                        kind: "call".to_string(),
                        s_value: Some("host_init_helper".to_string()),
                        out: Some("v0".to_string()),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                ],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "host_init_helper".to_string(),
                params: vec![],
                ops: vec![make_op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    eliminate_dead_functions(&mut ir);

    let retained: BTreeSet<&str> = ir.functions.iter().map(|func| func.name.as_str()).collect();
    assert!(retained.contains("molt_host_init"));
    assert!(retained.contains("host_init_helper"));
    let host_init = ir
        .functions
        .iter()
        .find(|func| func.name == "molt_host_init")
        .expect("molt_host_init must remain");
    assert!(
        host_init
            .ops
            .iter()
            .any(|op| op.s_value.as_deref() == Some("host_init_helper")),
        "molt_host_init must keep its transitive references",
    );
}

#[test]
fn eliminate_dead_functions_does_not_root_stdlib_from_partition_env() {
    let prior = std::env::var("MOLT_STDLIB_MODULE_SYMBOLS").ok();
    unsafe {
        std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", "[\"sys\"]");
    }

    let mut ir = SimpleIR {
        functions: vec![
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "entry".to_string(),
                params: vec![],
                ops: vec![make_op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_init_sys".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "call".to_string(),
                    s_value: Some("sys__helper".to_string()),
                    out: Some("v0".to_string()),
                    ..OpIR::default()
                }],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "sys__helper".to_string(),
                params: vec![],
                ops: vec![make_op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_init_json".to_string(),
                params: vec![],
                ops: vec![make_op("ret_void")],
                param_types: None,
                source_file: None,
                is_extern: false,
                codegen_partition: false,
                execution_context: Default::default(),
            },
        ],
        profile: None,
    };

    eliminate_dead_functions(&mut ir);

    match prior {
        Some(value) => unsafe { std::env::set_var("MOLT_STDLIB_MODULE_SYMBOLS", value) },
        None => unsafe { std::env::remove_var("MOLT_STDLIB_MODULE_SYMBOLS") },
    }

    let retained: BTreeSet<&str> = ir.functions.iter().map(|func| func.name.as_str()).collect();
    assert!(!retained.contains("molt_init_sys"));
    assert!(!retained.contains("sys__helper"));
    assert!(!retained.contains("molt_init_json"));
}
#[test]
fn generated_function_references_retain_exact_symbols_without_guessed_companions() {
    use molt_ir::tir::op_kinds_generated::SIMPLEIR_DEFINED_FUNCTION_REFERENCE_S_VALUE_KINDS;

    for &kind in SIMPLEIR_DEFINED_FUNCTION_REFERENCE_S_VALUE_KINDS {
        let function = |name: &str, ops: Vec<OpIR>| FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: name.to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        };
        for target in ["opaque_body", "opaque_body_poll", "undefined"] {
            let mut ir = SimpleIR {
                functions: vec![
                    function(
                        "entry",
                        vec![OpIR {
                            kind: kind.to_string(),
                            s_value: Some(target.to_string()),
                            ..OpIR::default()
                        }],
                    ),
                    function("opaque_body", vec![make_op("ret_void")]),
                    function("opaque_body_poll", vec![make_op("ret_void")]),
                    function("opaque_body_poll_poll", vec![make_op("ret_void")]),
                ],
                profile: None,
            };
            eliminate_dead_functions(&mut ir);
            let retained: Vec<&str> = ir
                .functions
                .iter()
                .map(|function| function.name.as_str())
                .collect();
            let expected = if target == "undefined" {
                vec!["entry"]
            } else {
                vec!["entry", target]
            };
            assert_eq!(retained, expected, "{kind} -> {target}");
        }
    }
}

#[test]
fn non_reference_operations_do_not_retain_defined_functions() {
    for kind in ["generator_create", "coro_create", "const_str"] {
        let function = |name: &str, ops: Vec<OpIR>| FunctionIR {
            return_abi: molt_ir::FunctionReturnAbi::Value,
            name: name.to_string(),
            params: Vec::new(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
            codegen_partition: false,
            execution_context: Default::default(),
        };
        let mut ir = SimpleIR {
            functions: vec![
                function(
                    "entry",
                    vec![OpIR {
                        kind: kind.to_string(),
                        s_value: Some("opaque_body".to_string()),
                        ..OpIR::default()
                    }],
                ),
                function("opaque_body", vec![make_op("ret_void")]),
                function("opaque_body_poll", vec![make_op("ret_void")]),
            ],
            profile: None,
        };
        eliminate_dead_functions(&mut ir);
        assert_eq!(ir.functions.len(), 1, "{kind}");
        assert_eq!(ir.functions[0].name, "entry", "{kind}");
    }
}
