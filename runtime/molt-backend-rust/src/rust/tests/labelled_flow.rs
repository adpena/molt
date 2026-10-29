use super::*;

fn op(kind: molt_ir::OpKind, args: &[&str], out: Option<&str>) -> OpIR {
    OpIR {
        args: (!args.is_empty()).then(|| args.iter().map(|arg| (*arg).to_string()).collect()),
        out: out.map(str::to_string),
        ..OpIR::new(kind)
    }
}

fn label(kind: molt_ir::OpKind, value: i64) -> OpIR {
    OpIR {
        value: Some(value),
        ..op(kind, &[], None)
    }
}

fn branch(condition: &str, target: i64) -> OpIR {
    OpIR {
        value: Some(target),
        ..op(molt_ir::OpKind::BrIf, &[condition], None)
    }
}

fn float(name: &str, value: f64) -> OpIR {
    OpIR {
        f_value: Some(value),
        ..op(molt_ir::OpKind::ConstFloat, &[], Some(name))
    }
}

fn boolean(name: &str, value: bool) -> OpIR {
    OpIR {
        value: Some(i64::from(value)),
        ..op(molt_ir::OpKind::ConstBool, &[], Some(name))
    }
}

fn store(name: &str, value: &str) -> OpIR {
    OpIR {
        var: Some(name.to_string()),
        ..op(molt_ir::OpKind::StoreVar, &[value], None)
    }
}

fn function(name: &str, params: &[(&str, &str)], ops: Vec<OpIR>) -> FunctionIR {
    FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
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
                    label(molt_ir::OpKind::Jump, 7),
                    float("unreachable_value", -1.0),
                    op(molt_ir::OpKind::Ret, &["unreachable_value"], None),
                    // This label is reachable despite the earlier lexical return.
                    label(molt_ir::OpKind::Label, 7),
                    float("answer", 7.0),
                    op(molt_ir::OpKind::Ret, &["answer"], None),
                ],
            ),
            function(
                "destination_work",
                &[("value", "float")],
                vec![
                    float("before_jump", 5.0),
                    store("value", "before_jump"),
                    label(molt_ir::OpKind::Goto, 20),
                    op(molt_ir::OpKind::Ret, &["value"], None),
                    label(molt_ir::OpKind::Label, 20),
                    // A jump must run this work, not return the last stored value.
                    float("after_jump", 9.0),
                    store("value", "after_jump"),
                    op(molt_ir::OpKind::Ret, &["value"], None),
                ],
            ),
            function(
                "conditional",
                &[("flag", "bool")],
                vec![
                    branch("flag", 11),
                    float("false_answer", 2.0),
                    op(molt_ir::OpKind::Ret, &["false_answer"], None),
                    label(molt_ir::OpKind::Label, 11),
                    float("true_answer", 1.0),
                    op(molt_ir::OpKind::Ret, &["true_answer"], None),
                ],
            ),
            function(
                "backward",
                &[("run", "bool"), ("value", "float")],
                vec![
                    label(molt_ir::OpKind::Label, 1),
                    branch("run", 2),
                    op(molt_ir::OpKind::Ret, &["value"], None),
                    label(molt_ir::OpKind::Label, 2),
                    boolean("stop", false),
                    float("step", 1.0),
                    op(molt_ir::OpKind::Add, &["value", "step"], Some("next_value")),
                    store("value", "next_value"),
                    store("run", "stop"),
                    label(molt_ir::OpKind::Jump, 1),
                ],
            ),
            function(
                "backward_swap",
                &[("run", "bool"), ("left", "float"), ("right", "float")],
                vec![
                    label(molt_ir::OpKind::Label, 1),
                    branch("run", 2),
                    op(molt_ir::OpKind::Ret, &["left"], None),
                    label(molt_ir::OpKind::Label, 2),
                    boolean("stop", false),
                    OpIR {
                        // Metadata deliberately differs from the SSA operand.
                        var: Some("right".to_string()),
                        ..op(molt_ir::OpKind::CopyVar, &["left"], Some("old_left"))
                    },
                    store("left", "right"),
                    store("right", "old_left"),
                    store("run", "stop"),
                    label(molt_ir::OpKind::Jump, 1),
                ],
            ),
            function(
                "structured_phi",
                &[("flag", "bool")],
                vec![
                    label(molt_ir::OpKind::Label, 0),
                    op(molt_ir::OpKind::If, &["flag"], None),
                    float("left", 3.0),
                    op(molt_ir::OpKind::Else, &[], None),
                    float("right", 7.0),
                    op(molt_ir::OpKind::EndIf, &[], None),
                    op(molt_ir::OpKind::Phi, &["left", "right"], Some("selected")),
                    label(molt_ir::OpKind::Jump, 20),
                    label(molt_ir::OpKind::Label, 20),
                    OpIR {
                        var: Some("flag".to_string()),
                        ..op(molt_ir::OpKind::LoadVar, &["selected"], Some("copied"))
                    },
                    op(molt_ir::OpKind::Ret, &["copied"], None),
                ],
            ),
            function(
                "structured_loop",
                &[],
                vec![
                    label(molt_ir::OpKind::Label, 0),
                    boolean("seed", true),
                    op(molt_ir::OpKind::LoopStart, &[], None),
                    op(molt_ir::OpKind::LoopIndexStart, &["seed"], Some("iterate")),
                    op(molt_ir::OpKind::If, &["iterate"], None),
                    label(molt_ir::OpKind::Jump, 3),
                    op(molt_ir::OpKind::Else, &[], None),
                    op(molt_ir::OpKind::LoopBreak, &[], None),
                    op(molt_ir::OpKind::EndIf, &[], None),
                    label(molt_ir::OpKind::Label, 3),
                    boolean("stop", false),
                    op(molt_ir::OpKind::LoopIndexNext, &["stop"], Some("iterate")),
                    op(molt_ir::OpKind::LoopContinue, &[], None),
                    op(molt_ir::OpKind::LoopEnd, &[], None),
                    op(molt_ir::OpKind::Ret, &["iterate"], None),
                ],
            ),
            function(
                "falloff",
                &[("flag", "bool")],
                vec![
                    branch("flag", 1),
                    float("answer", 42.0),
                    op(molt_ir::OpKind::Ret, &["answer"], None),
                    label(molt_ir::OpKind::Label, 1),
                    op(molt_ir::OpKind::ConstNone, &[], Some("implicit_none")),
                    op(molt_ir::OpKind::Ret, &["implicit_none"], None),
                ],
            ),
            function(
                "molt_main",
                &[],
                vec![
                    label(molt_ir::OpKind::Jump, 1),
                    op(molt_ir::OpKind::RetVoid, &[], None),
                    label(molt_ir::OpKind::Label, 1),
                    float("main_answer", 17.0),
                    op(molt_ir::OpKind::Print, &["main_answer"], None),
                    op(molt_ir::OpKind::RetVoid, &[], None),
                ],
            ),
        ],
        profile: None,
    };
    molt_tir::validate_simple_ir(&ir).expect("fixture transport must be valid");
    molt_ir::simple_verify::validate_simple_ir_control_flow(&ir)
        .expect("fixture must use the admitted canonical graph");
    let verification = molt_ir::simple_verify::verify_simple_ir(&ir);
    assert!(verification.is_ok(), "{:?}", verification.errors);
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
fn unreachable_terminators_trap_in_structured_and_labelled_flow() {
    let ir = SimpleIR {
        functions: vec![
            function(
                "structured_trap",
                &[],
                vec![op(molt_ir::OpKind::Unreachable, &[], None)],
            ),
            function(
                "labelled_trap",
                &[],
                vec![
                    label(molt_ir::OpKind::Jump, 1),
                    label(molt_ir::OpKind::Label, 1),
                    op(molt_ir::OpKind::Unreachable, &[], None),
                ],
            ),
            FunctionIR {
                return_abi: molt_ir::FunctionReturnAbi::Void,
                name: "molt_main".into(),
                ops: vec![op(molt_ir::OpKind::RetVoid, &[], None)],
                ..FunctionIR::default()
            },
        ],
        profile: None,
    };
    let mut backend = RustBackend::new();
    let source = backend.compile(&ir);
    assert!(
        backend.unsupported_ops.is_empty(),
        "{:?}",
        backend.unsupported_ops
    );
    let source = source.replacen(
        "fn main() {",
        r#"fn main() {
        assert!(std::panic::catch_unwind(|| structured_trap(&mut vec![])).is_err());
        assert!(std::panic::catch_unwind(|| labelled_trap(&mut vec![])).is_err());
    "#,
        1,
    );
    assert!(compile_and_run_emitted(&source, "unreachable_flow").is_empty());
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
                label(molt_ir::OpKind::Jump, 2),
                label(molt_ir::OpKind::Label, 1),
                float("left", 3.0),
                label(molt_ir::OpKind::Jump, 2),
                label(molt_ir::OpKind::Label, 2),
                op(molt_ir::OpKind::Phi, &["left", "right"], Some("selected")),
                op(molt_ir::OpKind::Ret, &["selected"], None),
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
        (
            vec![label(molt_ir::OpKind::Jump, 99)],
            "invalid-jump-target",
        ),
        (
            vec![op(molt_ir::OpKind::Jump, &[], None)],
            "malformed-label-reference",
        ),
        (
            vec![OpIR {
                s_value: Some("exit".to_string()),
                ..op(molt_ir::OpKind::Goto, &[], None)
            }],
            "malformed-label-reference",
        ),
        (
            vec![op(molt_ir::OpKind::Label, &[], None)],
            "malformed-label-definition",
        ),
        (
            vec![
                label(molt_ir::OpKind::Label, 1),
                label(molt_ir::OpKind::Label, 1),
            ],
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
        (label(molt_ir::OpKind::CheckException, 1), "exception"),
        (label(molt_ir::OpKind::AsyncWorkPoll, 1), "exception"),
        (
            op(molt_ir::OpKind::StateSwitch, &[], None),
            "async scheduler",
        ),
        (
            op(molt_ir::OpKind::ListNew, &[], Some("object")),
            "aliasing",
        ),
        (label(molt_ir::OpKind::Jump, 1), "unstructured control-flow"),
        (
            OpIR {
                value: Some(1),
                ..op(molt_ir::OpKind::BrIf, &["flag"], None)
            },
            "truthiness",
        ),
    ];
    for (unsupported, reason) in cases {
        let kind = unsupported.kind;
        let mut backend = RustBackend::new();
        let error = backend
            .compile_checked(&SimpleIR {
                functions: vec![function(
                    "runtime_gated_labels",
                    &[("flag", "bool")],
                    vec![
                        unsupported,
                        label(molt_ir::OpKind::Label, 1),
                        op(molt_ir::OpKind::RetVoid, &[], None),
                    ],
                )],
                profile: None,
            })
            .expect_err("a valid graph must not claim an unavailable runtime");
        assert!(
            error.contains("rejected before source generation"),
            "{kind}: {error}"
        );
        assert!(
            error.contains(kind.as_str()) && error.contains(reason),
            "{kind}: {error}"
        );
        assert!(backend.output.is_empty());
        assert!(backend.unsupported_ops.is_empty());
    }
}
