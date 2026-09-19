use super::*;

fn op(kind: &str, args: &[&str], out: Option<&str>) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: (!args.is_empty()).then(|| args.iter().map(|arg| (*arg).to_string()).collect()),
        out: out.map(str::to_string),
        ..OpIR::default()
    }
}

fn label(kind: &str, value: i64) -> OpIR {
    OpIR {
        value: Some(value),
        ..op(kind, &[], None)
    }
}

fn branch(condition: &str, target: i64) -> OpIR {
    OpIR {
        value: Some(target),
        ..op("br_if", &[condition], None)
    }
}

fn float(name: &str, value: f64) -> OpIR {
    OpIR {
        f_value: Some(value),
        ..op("const_float", &[], Some(name))
    }
}

fn boolean(name: &str, value: bool) -> OpIR {
    OpIR {
        value: Some(i64::from(value)),
        ..op("const_bool", &[], Some(name))
    }
}

fn store(name: &str, value: &str) -> OpIR {
    OpIR {
        var: Some(name.to_string()),
        ..op("store_var", &[value], None)
    }
}

fn function(name: &str, params: &[(&str, &str)], ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        name: name.to_string(),
        params: params.iter().map(|(name, _)| (*name).to_string()).collect(),
        param_types: Some(params.iter().map(|(_, ty)| (*ty).to_string()).collect()),
        ops,
        ..FunctionIR::default()
    }
}

#[test]
fn canonical_labelled_transfers_execute_targets_phis_backedges_and_returns() {
    let ir = SimpleIR {
        functions: vec![
            function(
                "forward",
                &[],
                vec![
                    label("jump", 7),
                    float("unreachable_value", -1.0),
                    op("ret", &["unreachable_value"], None),
                    // This label is reachable despite the earlier lexical return.
                    label("label", 7),
                    float("answer", 7.0),
                    op("ret", &["answer"], None),
                ],
            ),
            function(
                "destination_work",
                &[("value", "float")],
                vec![
                    float("before_jump", 5.0),
                    store("value", "before_jump"),
                    label("goto", 20),
                    op("ret", &["value"], None),
                    label("label", 20),
                    // A jump must run this work, not return the last stored value.
                    float("after_jump", 9.0),
                    store("value", "after_jump"),
                    op("ret", &["value"], None),
                ],
            ),
            function(
                "conditional",
                &[("flag", "bool")],
                vec![
                    branch("flag", 11),
                    float("false_answer", 2.0),
                    op("ret", &["false_answer"], None),
                    label("label", 11),
                    float("true_answer", 1.0),
                    op("ret", &["true_answer"], None),
                ],
            ),
            function(
                "backward",
                &[("run", "bool"), ("value", "float")],
                vec![
                    label("label", 1),
                    branch("run", 2),
                    op("ret", &["value"], None),
                    label("label", 2),
                    boolean("stop", false),
                    float("step", 1.0),
                    op("add", &["value", "step"], Some("next_value")),
                    store("value", "next_value"),
                    store("run", "stop"),
                    label("jump", 1),
                ],
            ),
            function(
                "backward_swap",
                &[("run", "bool"), ("left", "float"), ("right", "float")],
                vec![
                    label("label", 1),
                    branch("run", 2),
                    op("ret", &["left"], None),
                    label("label", 2),
                    boolean("stop", false),
                    OpIR {
                        // Metadata deliberately differs from the SSA operand.
                        var: Some("right".to_string()),
                        ..op("copy_var", &["left"], Some("old_left"))
                    },
                    store("left", "right"),
                    store("right", "old_left"),
                    store("run", "stop"),
                    label("jump", 1),
                ],
            ),
            function(
                "structured_phi",
                &[("flag", "bool")],
                vec![
                    label("label", 0),
                    op("if", &["flag"], None),
                    float("left", 3.0),
                    op("else", &[], None),
                    float("right", 7.0),
                    op("end_if", &[], None),
                    op("phi", &["left", "right"], Some("selected")),
                    label("jump", 20),
                    label("label", 20),
                    OpIR {
                        var: Some("flag".to_string()),
                        ..op("load_var", &["selected"], Some("copied"))
                    },
                    op("ret", &["copied"], None),
                ],
            ),
            function(
                "structured_loop",
                &[],
                vec![
                    label("label", 0),
                    boolean("seed", true),
                    op("loop_start", &[], None),
                    op("loop_index_start", &["seed"], Some("iterate")),
                    op("if", &["iterate"], None),
                    label("jump", 3),
                    op("else", &[], None),
                    op("loop_break", &[], None),
                    op("end_if", &[], None),
                    label("label", 3),
                    boolean("stop", false),
                    op("loop_index_next", &["stop"], Some("iterate")),
                    op("loop_continue", &[], None),
                    op("loop_end", &[], None),
                    op("ret", &["iterate"], None),
                ],
            ),
            function(
                "falloff",
                &[("flag", "bool")],
                vec![
                    branch("flag", 1),
                    float("answer", 42.0),
                    op("ret", &["answer"], None),
                    label("label", 1),
                ],
            ),
            function(
                "molt_main",
                &[],
                vec![
                    label("jump", 1),
                    op("ret_void", &[], None),
                    label("label", 1),
                    float("main_answer", 17.0),
                    op("print", &["main_answer"], None),
                    op("ret_void", &[], None),
                ],
            ),
        ],
        profile: None,
    };
    molt_tir::validate_simple_ir(&ir).expect("fixture transport must be valid");
    molt_ir::simple_verify::validate_simple_ir_control_flow(&ir)
        .expect("fixture must use the admitted canonical graph");
    // This proves the emitter, not target support: Rust still does not claim
    // unstructured flow, truthiness, or fallible-protocol runtime capabilities.
    let mut backend = RustBackend::new();
    let source = backend.compile(&ir);
    assert!(
        backend.unsupported_ops.is_empty(),
        "{:?}",
        backend.unsupported_ops
    );
    assert!(source.contains("match __molt_block"));
    // The harness only calls the emitted functions; their control flow and
    // value transport remain exactly the backend output.
    let mut source = source.replacen("fn main() {", "fn main() {\n    check_labelled_flow();", 1);
    source.push_str(
        r#"
fn check_labelled_flow() {
    #[track_caller]
    fn is_float(value: MoltValue, expected: f64) {
        assert!(
            matches!(&value, MoltValue::Float(actual) if *actual == expected),
            "expected Float({expected}), got {value:?}"
        );
    }
    is_float(forward(&mut vec![]), 7.0);
    let mut args = vec![MoltValue::Float(0.0)];
    is_float(destination_work(&mut args), 9.0);
    is_float(args[0].clone(), 9.0);
    for flag in [false, true] {
        let mut args = vec![MoltValue::Bool(flag)];
        is_float(conditional(&mut args), if flag { 1.0 } else { 2.0 });
        let mut args = vec![MoltValue::Bool(flag), MoltValue::Float(4.0)];
        is_float(backward(&mut args), if flag { 5.0 } else { 4.0 });
        assert!(matches!(args[0], MoltValue::Bool(false)));
        is_float(args[1].clone(), if flag { 5.0 } else { 4.0 });
        let mut args = vec![MoltValue::Bool(flag), MoltValue::Float(5.0), MoltValue::Float(9.0)];
        is_float(backward_swap(&mut args), if flag { 9.0 } else { 5.0 });
        assert!(matches!(args[0], MoltValue::Bool(false)));
        is_float(args[1].clone(), if flag { 9.0 } else { 5.0 });
        is_float(args[2].clone(), if flag { 5.0 } else { 9.0 });
        let mut args = vec![MoltValue::Bool(flag)];
        is_float(structured_phi(&mut args), if flag { 3.0 } else { 7.0 });
    }
    assert!(matches!(structured_loop(&mut vec![]), MoltValue::Bool(false)));
    assert!(matches!(falloff(&mut vec![MoltValue::Bool(true)]), MoltValue::None));
    is_float(falloff(&mut vec![MoltValue::Bool(false)]), 42.0);
}
"#,
    );
    assert_eq!(
        compile_and_run_emitted(&source, "labelled_scalar_flow").trim(),
        "17.0"
    );
}

#[test]
fn labelled_flow_rejects_residual_unstructured_phi_without_choosing_an_input() {
    let ir = SimpleIR {
        functions: vec![function(
            "labelled_phi",
            &[("flag", "bool")],
            vec![
                branch("flag", 1),
                float("right", 7.0),
                label("jump", 2),
                label("label", 1),
                float("left", 3.0),
                label("jump", 2),
                label("label", 2),
                op("phi", &["left", "right"], Some("selected")),
                op("ret", &["selected"], None),
            ],
        )],
        profile: None,
    };
    molt_ir::simple_verify::validate_simple_ir_control_flow(&ir)
        .expect("labels and branches are valid even though this PHI is not lowered");
    let mut backend = RustBackend::new();
    let _source = backend.compile(&ir);
    assert!(
        backend.unsupported_ops.iter().any(|diagnostic| {
            diagnostic.contains("`phi`") && diagnostic.contains("unresolved predecessor value")
        }),
        "{:?}",
        backend.unsupported_ops
    );
}

#[test]
fn labelled_flow_rejects_malformed_namespaces_before_emission() {
    let cases = [
        (vec![label("jump", 99)], "invalid-jump-target"),
        (vec![op("jump", &[], None)], "malformed-label-reference"),
        (
            vec![OpIR {
                s_value: Some("exit".to_string()),
                ..op("goto", &[], None)
            }],
            "malformed-label-reference",
        ),
        (vec![op("label", &[], None)], "malformed-label-definition"),
        (
            vec![label("label", 1), label("label", 1)],
            "duplicate-label-definition",
        ),
    ];
    for (ops, diagnostic) in cases {
        let mut backend = RustBackend::new();
        let error = backend
            .compile_checked(&SimpleIR {
                functions: vec![function("malformed_labels", &[], ops)],
                profile: None,
            })
            .expect_err("invalid namespace must never reach SSA or source emission");
        assert!(error.contains(diagnostic), "{diagnostic}: {error}");
        assert!(backend.output.is_empty());
        assert!(backend.unsupported_ops.is_empty());
    }
}

#[test]
fn labelled_flow_does_not_bypass_runtime_capability_admission() {
    let cases = [
        (label("check_exception", 1), "exception"),
        (label("async_work_poll", 1), "exception"),
        (op("state_switch", &[], None), "async scheduler"),
        (op("list_new", &[], Some("object")), "aliasing"),
        (label("jump", 1), "unstructured control-flow"),
        (
            OpIR {
                value: Some(1),
                ..op("br_if", &["flag"], None)
            },
            "truthiness",
        ),
    ];
    for (unsupported, reason) in cases {
        let kind = unsupported.kind.clone();
        let mut backend = RustBackend::new();
        let error = backend
            .compile_checked(&SimpleIR {
                functions: vec![function(
                    "runtime_gated_labels",
                    &[("flag", "bool")],
                    vec![unsupported, label("label", 1), op("ret_void", &[], None)],
                )],
                profile: None,
            })
            .expect_err("a valid graph must not claim an unavailable runtime");
        assert!(
            error.contains("rejected before source generation"),
            "{kind}: {error}"
        );
        assert!(
            error.contains(&kind) && error.contains(reason),
            "{kind}: {error}"
        );
        assert!(backend.output.is_empty());
        assert!(backend.unsupported_ops.is_empty());
    }
}
