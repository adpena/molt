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

fn make_legacy_scalar_hint_ir() -> FunctionIR {
    let ops = vec![
        OpIR {
            kind: "call_method".to_string(),
            args: Some(vec!["obj".into()]),
            s_value: Some("size".into()),
            out: Some("method_result".into()),
            type_hint: Some("int".into()),
            ..Default::default()
        },
        OpIR {
            kind: "index".to_string(),
            args: Some(vec!["obj".into(), "idx".into()]),
            out: Some("indexed".into()),
            type_hint: Some("int".into()),
            ..Default::default()
        },
        OpIR {
            kind: "call".to_string(),
            args: Some(vec!["method_result".into(), "indexed".into()]),
            s_value: Some("opaque".into()),
            out: Some("called".into()),
            type_hint: Some("int".into()),
            ..Default::default()
        },
        OpIR {
            kind: "ret".to_string(),
            args: Some(vec!["called".into()]),
            ..Default::default()
        },
    ];

    FunctionIR {
        name: "legacy_scalar_hints".to_string(),
        ops,
        params: vec!["obj".into(), "idx".into()],
        param_types: None,
        source_file: None,
        is_extern: false,
    }
}

fn roundtrip(ir: &FunctionIR) -> Vec<OpIR> {
    let tir_func = molt_backend::tir::lower_from_simple::lower_to_tir(ir);
    molt_backend::tir::lower_to_simple::lower_to_simple_ir(&tir_func)
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

    let scalar_hint_roundtrip = roundtrip(&make_legacy_scalar_hint_ir());
    assert!(
        scalar_hint_roundtrip
            .iter()
            .filter(|op| matches!(op.kind.as_str(), "call" | "call_method" | "index"))
            .all(|op| op.type_hint.is_none()),
        "typed TIR roundtrip should not reintroduce legacy scalar hints for opaque ops: {:?}",
        scalar_hint_roundtrip
            .iter()
            .filter(|op| matches!(op.kind.as_str(), "call" | "call_method" | "index"))
            .map(|op| (op.kind.clone(), op.type_hint.clone()))
            .collect::<Vec<_>>(),
    );
}

#[test]
fn roundtrip_preserves_structural_source_site() {
    let ir = FunctionIR {
        name: "source_site".to_string(),
        ops: vec![
            OpIR {
                kind: "line".to_string(),
                value: Some(12),
                source_line: Some(12),
                col_offset: Some(4),
                end_col_offset: Some(18),
                ..Default::default()
            },
            OpIR {
                kind: "const".to_string(),
                value: Some(41),
                out: Some("x".into()),
                ..Default::default()
            },
            OpIR {
                kind: "add".to_string(),
                args: Some(vec!["x".into(), "1".into()]),
                out: Some("y".into()),
                ..Default::default()
            },
            OpIR {
                kind: "ret".to_string(),
                args: Some(vec!["y".into()]),
                ..Default::default()
            },
        ],
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
    };

    let tir_func = molt_backend::tir::lower_from_simple::lower_to_tir(&ir);
    let mut sites = tir_func
        .blocks
        .values()
        .flat_map(|block| block.ops.iter())
        .filter_map(|op| op.source_site());
    let site = sites
        .find(|site| site.line == 12)
        .expect("TIR ops should carry the active source line");
    assert_eq!(site.col, Some(4));
    assert_eq!(site.end_col, Some(18));

    let roundtripped = molt_backend::tir::lower_to_simple::lower_to_simple_ir(&tir_func);
    let const_op = roundtripped
        .iter()
        .find(|op| op.kind == "const" && op.out.as_deref() == Some("x"))
        .expect("roundtrip should retain the const op");
    assert_eq!(const_op.source_line, Some(12));
    assert_eq!(const_op.col_offset, Some(4));
    assert_eq!(const_op.end_col_offset, Some(18));

    let add_op = roundtripped
        .iter()
        .find(|op| op.kind == "add" && op.out.as_deref() == Some("y"))
        .expect("roundtrip should retain the add op");
    assert_eq!(add_op.source_line, Some(12));
    assert_eq!(add_op.col_offset, Some(4));
    assert_eq!(add_op.end_col_offset, Some(18));
}
