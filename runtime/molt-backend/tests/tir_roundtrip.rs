use molt_backend::{FunctionIR, OpIR};

fn make_comprehension_ir() -> FunctionIR {
    let ops = vec![
        OpIR {
            value: Some(1),
            out: Some("v1".into()),
            ..OpIR::new(molt_ir::OpKind::Const)
        },
        OpIR {
            value: Some(2),
            out: Some("v2".into()),
            ..OpIR::new(molt_ir::OpKind::Const)
        },
        OpIR {
            value: Some(3),
            out: Some("v3".into()),
            ..OpIR::new(molt_ir::OpKind::Const)
        },
        OpIR {
            args: Some(vec!["v1".into(), "v2".into(), "v3".into()]),
            out: Some("v4".into()),
            type_hint: Some("list".into()),
            ..OpIR::new(molt_ir::OpKind::ListNew)
        },
        OpIR {
            args: Some(vec!["v4".into()]),
            out: Some("v5".into()),
            ..OpIR::new(molt_ir::OpKind::Iter)
        },
        OpIR {
            args: Some(vec![]),
            out: Some("v6".into()),
            type_hint: Some("list".into()),
            ..OpIR::new(molt_ir::OpKind::ListNew)
        },
        OpIR {
            value: Some(0),
            out: Some("v7".into()),
            ..OpIR::new(molt_ir::OpKind::Const)
        },
        OpIR {
            value: Some(1),
            out: Some("v8".into()),
            ..OpIR::new(molt_ir::OpKind::Const)
        },
        OpIR::new(molt_ir::OpKind::LoopStart),
        OpIR {
            args: Some(vec!["v5".into()]),
            out: Some("v9".into()),
            ..OpIR::new(molt_ir::OpKind::IterNext)
        },
        OpIR {
            args: Some(vec!["v9".into(), "v8".into()]),
            out: Some("v10".into()),
            ..OpIR::new(molt_ir::OpKind::Index)
        },
        OpIR {
            args: Some(vec!["v10".into()]),
            ..OpIR::new(molt_ir::OpKind::LoopBreakIfTrue)
        },
        OpIR {
            args: Some(vec!["v9".into(), "v7".into()]),
            out: Some("v11".into()),
            ..OpIR::new(molt_ir::OpKind::Index)
        },
        OpIR {
            args: Some(vec!["v6".into(), "v11".into()]),
            out: Some("none".into()),
            type_hint: Some("list".into()),
            ..OpIR::new(molt_ir::OpKind::ListAppend)
        },
        OpIR::new(molt_ir::OpKind::LoopContinue),
        OpIR::new(molt_ir::OpKind::LoopEnd),
        OpIR {
            args: Some(vec!["v6".into()]),
            ..OpIR::new(molt_ir::OpKind::Ret)
        },
    ];

    FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "test_comp".to_string(),
        ops,
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
    }
}

fn make_legacy_scalar_hint_ir() -> FunctionIR {
    let ops = vec![
        OpIR {
            args: Some(vec!["obj".into()]),
            s_value: Some("size".into()),
            out: Some("method_result".into()),
            type_hint: Some("int".into()),
            ..OpIR::new(molt_ir::OpKind::CallMethod)
        },
        OpIR {
            args: Some(vec!["obj".into(), "idx".into()]),
            out: Some("indexed".into()),
            type_hint: Some("int".into()),
            ..OpIR::new(molt_ir::OpKind::Index)
        },
        OpIR {
            args: Some(vec!["method_result".into(), "indexed".into()]),
            s_value: Some("opaque".into()),
            out: Some("called".into()),
            type_hint: Some("int".into()),
            ..OpIR::new(molt_ir::OpKind::Call)
        },
        OpIR {
            args: Some(vec!["called".into()]),
            ..OpIR::new(molt_ir::OpKind::Ret)
        },
    ];

    FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "legacy_scalar_hints".to_string(),
        ops,
        params: vec!["obj".into(), "idx".into()],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
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
        roundtripped
            .iter()
            .filter(|op| op.kind == molt_ir::OpKind::Const)
            .count(),
        5,
        "roundtrip should keep the comprehension constants intact",
    );
    assert_eq!(
        roundtripped
            .iter()
            .filter(|op| op.kind == molt_ir::OpKind::ListNew)
            .count(),
        2,
        "roundtrip should keep both list allocations intact",
    );
    assert_eq!(
        roundtripped
            .iter()
            .filter(|op| op.kind == molt_ir::OpKind::ListAppend)
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
            .filter(|op| matches!(
                op.kind,
                molt_ir::OpKind::ListNew | molt_ir::OpKind::ListAppend
            ))
            .all(|op| op.type_hint.is_none()),
        "typed TIR roundtrip should not reintroduce legacy container hints: {:?}",
        roundtripped
            .iter()
            .filter(|op| matches!(
                op.kind,
                molt_ir::OpKind::ListNew | molt_ir::OpKind::ListAppend
            ))
            .map(|op| op.type_hint.clone())
            .collect::<Vec<_>>(),
    );

    let scalar_hint_roundtrip = roundtrip(&make_legacy_scalar_hint_ir());
    assert!(
        scalar_hint_roundtrip
            .iter()
            .filter(|op| matches!(
                op.kind,
                molt_ir::OpKind::Call | molt_ir::OpKind::CallMethod | molt_ir::OpKind::Index
            ))
            .all(|op| op.type_hint.is_none()),
        "typed TIR roundtrip should not reintroduce legacy scalar hints for opaque ops: {:?}",
        scalar_hint_roundtrip
            .iter()
            .filter(|op| matches!(
                op.kind,
                molt_ir::OpKind::Call | molt_ir::OpKind::CallMethod | molt_ir::OpKind::Index
            ))
            .map(|op| (op.kind, op.type_hint.clone()))
            .collect::<Vec<_>>(),
    );
}

#[test]
fn roundtrip_preserves_structural_source_site() {
    let ir = FunctionIR {
        return_abi: molt_ir::FunctionReturnAbi::Value,
        name: "source_site".to_string(),
        ops: vec![
            OpIR {
                value: Some(12),
                source_line: Some(12),
                col_offset: Some(4),
                end_col_offset: Some(18),
                ..OpIR::new(molt_ir::OpKind::Line)
            },
            OpIR {
                value: Some(41),
                out: Some("x".into()),
                ..OpIR::new(molt_ir::OpKind::Const)
            },
            OpIR {
                args: Some(vec!["x".into(), "1".into()]),
                out: Some("y".into()),
                ..OpIR::new(molt_ir::OpKind::Add)
            },
            OpIR {
                args: Some(vec!["y".into()]),
                ..OpIR::new(molt_ir::OpKind::Ret)
            },
        ],
        params: vec![],
        param_types: None,
        source_file: None,
        is_extern: false,
        codegen_partition: false,
        parameter_custody: Vec::new(),
        execution_context: Default::default(),
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
        .find(|op| op.kind == molt_ir::OpKind::Const && op.out.as_deref() == Some("x"))
        .expect("roundtrip should retain the const op");
    assert_eq!(const_op.source_line, Some(12));
    assert_eq!(const_op.col_offset, Some(4));
    assert_eq!(const_op.end_col_offset, Some(18));

    let add_op = roundtripped
        .iter()
        .find(|op| op.kind == molt_ir::OpKind::Add && op.out.as_deref() == Some("y"))
        .expect("roundtrip should retain the add op");
    assert_eq!(add_op.source_line, Some(12));
    assert_eq!(add_op.col_offset, Some(4));
    assert_eq!(add_op.end_col_offset, Some(18));
}
