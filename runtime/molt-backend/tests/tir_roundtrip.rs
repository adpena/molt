use molt_backend::{FunctionIR, OpIR};

fn make_comprehension_ir() -> FunctionIR {
    let ops = vec![
        OpIR {
            kind: "const".to_string(),
            value: Some(1),
            out: Some("v1".into()),
            fast_int: Some(true),
            ..Default::default()
        },
        OpIR {
            kind: "const".to_string(),
            value: Some(2),
            out: Some("v2".into()),
            fast_int: Some(true),
            ..Default::default()
        },
        OpIR {
            kind: "const".to_string(),
            value: Some(3),
            out: Some("v3".into()),
            fast_int: Some(true),
            ..Default::default()
        },
        OpIR {
            kind: "list_new".to_string(),
            args: Some(vec!["v1".into(), "v2".into(), "v3".into()]),
            out: Some("v4".into()),
            type_hint: Some("list".into()),
            ..Default::default()
        },
        OpIR {
            kind: "iter".to_string(),
            args: Some(vec!["v4".into()]),
            out: Some("v5".into()),
            ..Default::default()
        },
        OpIR {
            kind: "list_new".to_string(),
            args: Some(vec![]),
            out: Some("v6".into()),
            type_hint: Some("list".into()),
            ..Default::default()
        },
        OpIR {
            kind: "const".to_string(),
            value: Some(0),
            out: Some("v7".into()),
            fast_int: Some(true),
            ..Default::default()
        },
        OpIR {
            kind: "const".to_string(),
            value: Some(1),
            out: Some("v8".into()),
            fast_int: Some(true),
            ..Default::default()
        },
        OpIR {
            kind: "loop_start".to_string(),
            ..Default::default()
        },
        OpIR {
            kind: "iter_next".to_string(),
            args: Some(vec!["v5".into()]),
            out: Some("v9".into()),
            ..Default::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["v9".into(), "v8".into()]),
            out: Some("v10".into()),
            ..Default::default()
        },
        OpIR {
            kind: "loop_break_if_true".to_string(),
            args: Some(vec!["v10".into()]),
            ..Default::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["v9".into(), "v7".into()]),
            out: Some("v11".into()),
            ..Default::default()
        },
        OpIR {
            kind: "list_append".to_string(),
            args: Some(vec!["v6".into(), "v11".into()]),
            out: Some("none".into()),
            type_hint: Some("list".into()),
            ..Default::default()
        },
        OpIR {
            kind: "loop_continue".to_string(),
            ..Default::default()
        },
        OpIR {
            kind: "loop_end".to_string(),
            ..Default::default()
        },
        OpIR {
            kind: "ret".to_string(),
            args: Some(vec!["v6".into()]),
            ..Default::default()
        },
    ];

    FunctionIR {
        name: "test_comp".to_string(),
        ops,
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
    }
}

fn roundtrip(ir: &FunctionIR) -> Vec<OpIR> {
    let tir_func = molt_backend::tir::lower_from_simple::lower_to_tir(ir);
    let type_map = molt_backend::tir::type_refine::extract_type_map(&tir_func);
    molt_backend::tir::lower_to_simple::lower_to_simple_ir(&tir_func, &type_map)
}

#[test]
fn roundtrip_preserves_comprehension_shape_without_transport_hints() {
    let roundtripped = roundtrip(&make_comprehension_ir());

    assert_eq!(
        roundtripped.iter().filter(|op| op.kind == "const").count(),
        5,
        "roundtrip should keep the comprehension constants intact",
    );
    assert_eq!(
        roundtripped
            .iter()
            .filter(|op| op.kind == "list_new")
            .count(),
        2,
        "roundtrip should keep both list allocations intact",
    );
    assert_eq!(
        roundtripped
            .iter()
            .filter(|op| op.kind == "list_append")
            .count(),
        1,
        "roundtrip should keep the list_append transport op intact",
    );
}

#[test]
fn roundtrip_does_not_reseed_legacy_transport_hints() {
    let roundtripped = roundtrip(&make_comprehension_ir());

    assert!(
        roundtripped
            .iter()
            .filter(|op| op.kind == "const")
            .all(|op| op.fast_int.is_none()),
        "typed TIR roundtrip should not reintroduce legacy fast_int hints: {:?}",
        roundtripped
            .iter()
            .filter(|op| op.kind == "const")
            .map(|op| op.fast_int)
            .collect::<Vec<_>>(),
    );
    assert!(
        roundtripped
            .iter()
            .filter(|op| matches!(op.kind.as_str(), "list_new" | "list_append"))
            .all(|op| op.type_hint.is_none()),
        "typed TIR roundtrip should not reintroduce legacy container hints: {:?}",
        roundtripped
            .iter()
            .filter(|op| matches!(op.kind.as_str(), "list_new" | "list_append"))
            .map(|op| op.type_hint.clone())
            .collect::<Vec<_>>(),
    );
}
