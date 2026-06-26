use super::*;

fn op_shapes(
    ops: &[OpIR],
) -> Vec<(
    String,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Vec<String>>,
)> {
    ops.iter()
        .map(|op| {
            (
                op.kind.clone(),
                op.value,
                op.out.clone(),
                op.var.clone(),
                op.s_value.clone(),
                op.args.clone(),
            )
        })
        .collect()
}

#[test]
fn try_elision_preserves_try_finally_cleanup_shape() {
    let mut ops = vec![
        OpIR {
            kind: "exception_push".to_string(),
            out: Some("none".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_start".to_string(),
            value: Some(10),
            ..OpIR::default()
        },
        OpIR {
            kind: "line".to_string(),
            value: Some(1),
            ..OpIR::default()
        },
        OpIR {
            kind: "check_exception".to_string(),
            value: Some(10),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_end".to_string(),
            value: Some(10),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".to_string(),
            value: Some(11),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(10),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(11),
            ..OpIR::default()
        },
        OpIR {
            kind: "const_none".to_string(),
            out: Some("finally_value".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "exception_pop".to_string(),
            out: Some("none".to_string()),
            ..OpIR::default()
        },
    ];

    let original = ops.clone();
    elide_useless_try_blocks(&mut ops);

    assert_eq!(
        op_shapes(&ops),
        op_shapes(&original),
        "try/finally must not use the try/except-only elision path"
    );
}

#[test]
fn try_except_elision_drops_body_checks_to_removed_handler_labels() {
    let mut ops = vec![
        OpIR {
            kind: "exception_push".to_string(),
            out: Some("none".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_start".to_string(),
            value: Some(10),
            ..OpIR::default()
        },
        OpIR {
            kind: "const".to_string(),
            value: Some(1),
            out: Some("x".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "check_exception".to_string(),
            value: Some(10),
            ..OpIR::default()
        },
        OpIR {
            kind: "store_var".to_string(),
            var: Some("total".to_string()),
            args: Some(vec!["x".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_end".to_string(),
            value: Some(10),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".to_string(),
            value: Some(11),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(10),
            ..OpIR::default()
        },
        OpIR {
            kind: "exception_last".to_string(),
            out: Some("exc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "exception_match_builtin".to_string(),
            value: Some(3),
            s_value: Some("ValueError".to_string()),
            args: Some(vec!["exc".to_string()]),
            out: Some("matched".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(11),
            ..OpIR::default()
        },
        OpIR {
            kind: "exception_pop".to_string(),
            out: Some("none".to_string()),
            ..OpIR::default()
        },
    ];

    elide_useless_try_blocks(&mut ops);

    let kinds: Vec<&str> = ops.iter().map(|op| op.kind.as_str()).collect();
    assert_eq!(kinds, vec!["const", "store_var"]);
    assert!(
        ops.iter()
            .all(|op| !(op.kind == "check_exception" && op.value == Some(10))),
        "eliding a safe try/except body must not leave stale handler checks: {ops:?}"
    );
}

#[test]
fn try_except_elision_keeps_transport_hinted_unknown_add() {
    let mut add = OpIR {
        kind: "add".to_string(),
        args: Some(vec!["left".to_string(), "right".to_string()]),
        out: Some("sum".to_string()),
        ..OpIR::default()
    };
    add.fast_int = Some(true);
    let mut func = FunctionIR {
        name: "transport_hint_try_body".to_string(),
        params: vec!["left".to_string(), "right".to_string()],
        param_types: None,
        source_file: None,
        is_extern: false,
        ops: vec![
            OpIR {
                kind: "exception_push".to_string(),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_start".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            add,
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("result".to_string()),
                args: Some(vec!["sum".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "jump".to_string(),
                value: Some(11),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_last".to_string(),
                out: Some("exc".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_match_builtin".to_string(),
                value: Some(3),
                s_value: Some("ValueError".to_string()),
                args: Some(vec!["exc".to_string()]),
                out: Some("matched".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(11),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_pop".to_string(),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
        ],
    };

    elide_useless_try_blocks_for_function(&mut func);

    assert!(
        func.ops.iter().any(|op| op.kind == "exception_push")
            && func.ops.iter().any(|op| op.kind == "add"),
        "transport hints alone must not elide try/except around Python arithmetic: {:?}",
        func.ops
    );
}

#[test]
fn try_except_elision_uses_typed_int_body_without_transport_hints() {
    let mut func = FunctionIR {
        name: "typed_int_try_body".to_string(),
        params: vec!["left".to_string(), "right".to_string()],
        param_types: Some(vec!["int".to_string(), "int".to_string()]),
        source_file: None,
        is_extern: false,
        ops: vec![
            OpIR {
                kind: "exception_push".to_string(),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_start".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["left".to_string(), "right".to_string()]),
                out: Some("sum".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "check_exception".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "store_var".to_string(),
                var: Some("result".to_string()),
                args: Some(vec!["sum".to_string()]),
                ..OpIR::default()
            },
            OpIR {
                kind: "try_end".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "jump".to_string(),
                value: Some(11),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(10),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_last".to_string(),
                out: Some("exc".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_match_builtin".to_string(),
                value: Some(3),
                s_value: Some("ValueError".to_string()),
                args: Some(vec!["exc".to_string()]),
                out: Some("matched".to_string()),
                ..OpIR::default()
            },
            OpIR {
                kind: "label".to_string(),
                value: Some(11),
                ..OpIR::default()
            },
            OpIR {
                kind: "exception_pop".to_string(),
                out: Some("none".to_string()),
                ..OpIR::default()
            },
        ],
    };

    elide_useless_try_blocks_for_function(&mut func);

    let kinds: Vec<&str> = func.ops.iter().map(|op| op.kind.as_str()).collect();
    assert_eq!(kinds, vec!["add", "store_var"]);
}

#[test]
fn try_except_elision_aborts_when_body_branches_to_removed_wrapper_label() {
    let mut ops = vec![
        OpIR {
            kind: "exception_push".to_string(),
            out: Some("none".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_start".to_string(),
            value: Some(10),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".to_string(),
            value: Some(11),
            ..OpIR::default()
        },
        OpIR {
            kind: "try_end".to_string(),
            value: Some(10),
            ..OpIR::default()
        },
        OpIR {
            kind: "jump".to_string(),
            value: Some(11),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(10),
            ..OpIR::default()
        },
        OpIR {
            kind: "exception_last".to_string(),
            out: Some("exc".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "exception_match_builtin".to_string(),
            value: Some(3),
            s_value: Some("ValueError".to_string()),
            args: Some(vec!["exc".to_string()]),
            out: Some("matched".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(11),
            ..OpIR::default()
        },
        OpIR {
            kind: "exception_pop".to_string(),
            out: Some("none".to_string()),
            ..OpIR::default()
        },
    ];

    let original = ops.clone();
    elide_useless_try_blocks(&mut ops);

    assert_eq!(
        op_shapes(&ops),
        op_shapes(&original),
        "try/except elision must be CFG-closed when wrapper-local labels are removed"
    );
}

#[test]
fn rewrite_phi_to_store_load_rewrites_merge_phi() {
    let mut ops = vec![
        OpIR {
            kind: "const_bool".to_string(),
            value: Some(1),
            out: Some("cond".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "if".to_string(),
            args: Some(vec!["cond".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "const".to_string(),
            value: Some(1),
            out: Some("then_val".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "else".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "const".to_string(),
            value: Some(2),
            out: Some("else_val".to_string()),
            ..OpIR::default()
        },
        OpIR {
            kind: "end_if".to_string(),
            ..OpIR::default()
        },
        OpIR {
            kind: "phi".to_string(),
            out: Some("merged".to_string()),
            args: Some(vec!["then_val".to_string(), "else_val".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "ret".to_string(),
            var: Some("merged".to_string()),
            ..OpIR::default()
        },
    ];

    rewrite_phi_to_store_load(&mut ops);

    assert!(
        ops.iter().all(|op| op.kind != "phi"),
        "phi should be eliminated: {ops:?}"
    );
    assert!(
        ops.iter().any(|op| {
            op.kind == "store_var"
                && op.var.as_deref() == Some("_phi_merged")
                && op
                    .args
                    .as_ref()
                    .is_some_and(|args| args.len() == 1 && args[0] == "then_val")
        }),
        "then branch should store merged value"
    );
    assert!(
        ops.iter().any(|op| {
            op.kind == "store_var"
                && op.var.as_deref() == Some("_phi_merged")
                && op
                    .args
                    .as_ref()
                    .is_some_and(|args| args.len() == 1 && args[0] == "else_val")
        }),
        "else branch should store merged value"
    );
    assert!(
        ops.iter().any(|op| {
            op.kind == "load_var"
                && op.var.as_deref() == Some("_phi_merged")
                && op.out.as_deref() == Some("merged")
        }),
        "merged phi should become load_var"
    );
}
