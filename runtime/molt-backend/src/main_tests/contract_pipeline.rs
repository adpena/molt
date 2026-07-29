use super::*;

#[test]
fn fact_graph_cli_contract_requires_output_and_function_pair() {
    let err = validate_fact_graph_cli_contract(Some("graph.json"), None, false)
        .expect_err("unpaired fact graph flags must fail closed");
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    assert!(
        err.to_string()
            .contains("--fact-graph-output and --fact-graph-function")
    );
}

#[test]
fn fact_graph_cli_contract_rejects_rust_target() {
    let err = validate_fact_graph_cli_contract(Some("graph.json"), Some("molt_main"), true)
        .expect_err("rust target fact graph request must fail closed");
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    assert!(err.to_string().contains("rust target"));
}

#[test]
fn luau_tir_module_pipeline_inlines_direct_local_calls() {
    let callee = FunctionIR {
        name: "luau_add1".to_string(),
        params: vec!["x".to_string()],
        param_types: Some(vec!["int".to_string()]),
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                value: Some(1),
                out: Some("one".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["x".to_string(), "one".to_string()]),
                out: Some("sum".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["sum".to_string()]),
                ..OpIR::default()
            },
        ],
        source_file: None,
        is_extern: false,
        execution_context: Default::default(),
    };
    let caller = FunctionIR {
        name: "molt_main".to_string(),
        params: Vec::new(),
        param_types: None,
        ops: vec![
            OpIR {
                kind: "const".to_string(),
                value: Some(41),
                out: Some("arg".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "call".to_string(),
                s_value: Some("luau_add1".to_string()),
                args: Some(vec!["arg".to_string()]),
                out: Some("result".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["result".to_string()]),
                ..OpIR::default()
            },
        ],
        source_file: None,
        is_extern: false,
        execution_context: Default::default(),
    };
    let mut ir = SimpleIR {
        functions: vec![caller, callee],
        profile: None,
    };

    let stats = run_luau_tir_module_pipeline(&mut ir).expect("luau module pipeline");

    assert_eq!(stats.functions, 2);
    assert!(
        ir.functions
            .iter()
            .flat_map(|function| &function.ops)
            .all(|op| op.kind != "async_work_poll"),
        "Luau's target pipeline must not inject a native pending-call/eval-breaker boundary"
    );
    assert!(
        stats.module_changed >= 1,
        "direct call inlining must report at least one changed function"
    );
    let main = ir
        .functions
        .iter()
        .find(|func| func.name == "molt_main")
        .expect("molt_main");
    assert!(
        main.ops
            .iter()
            .all(|op| !(op.kind == "call" && op.s_value.as_deref() == Some("luau_add1"))),
        "Luau module phase must inline direct local calls instead of leaving a call boundary: {:?}",
        main.ops
    );
}

#[cfg(feature = "rust-backend")]
#[test]
fn rust_source_for_ir_rejects_unknown_ops_at_generated_semantic_authority() {
    let ir = SimpleIR {
        functions: vec![FunctionIR {
            name: "molt_main".to_string(),
            params: vec![],
            ops: vec![OpIR {
                kind: "unsupported_for_rust_target_test".to_string(),
                out: Some("v0".to_string()),
                ..OpIR::default()
            }],
            param_types: None,
            source_file: None,
            is_extern: false,
            execution_context: Default::default(),
        }],
        profile: None,
    };

    let err = rust_source_for_ir(&ir).expect_err("Rust target must reject unknown operations");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        err.to_string()
            .contains("unclassified in the generated runtime semantic authority"),
        "unexpected error: {err}"
    );
}

#[cfg(feature = "rust-backend")]
#[test]
fn rust_source_for_ir_prunes_unreachable_stub_markers() {
    let ir = SimpleIR {
        functions: vec![
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
            FunctionIR {
                name: "dead_stdlib_helper".to_string(),
                params: vec![],
                ops: vec![OpIR {
                    kind: "unsupported_for_rust_target_test".to_string(),
                    out: Some("v0".to_string()),
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

    let source = rust_source_for_ir(&ir).expect("dead stubs must be pruned before Rust emit");
    assert!(source.contains("fn molt_main("));
    assert!(!source.contains("dead_stdlib_helper"));
    assert!(!source.contains("MOLT_STUB"));
}
