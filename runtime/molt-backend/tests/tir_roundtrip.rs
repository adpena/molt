/// Test that lower_to_tir → lower_to_simple_ir roundtrip preserves transport
/// compatibility metadata while the SimpleIR surface still exists.
///
/// The canonical backend contract is representation-aware TIR/LIR, but some
/// transport consumers still read compatibility hints on SimpleIR ops. These
/// tests make sure the roundtrip does not silently erase those fields.
use molt_backend::{FunctionIR, OpIR};

fn make_comprehension_ir() -> FunctionIR {
    // Simulate: [x for x in [1, 2, 3]]
    // This is a simplified version of what the frontend generates.
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

#[test]
fn roundtrip_preserves_integer_compatibility_hint() {
    let ir = make_comprehension_ir();

    // Check original carries integer compatibility hints.
    let orig_consts: Vec<_> = ir
        .ops
        .iter()
        .filter(|op| op.kind == "const" && op.fast_int == Some(true))
        .collect();
    assert!(
        !orig_consts.is_empty(),
        "original IR should carry integer compatibility hints on consts"
    );

    // Round-trip through TIR
    let tir_func = molt_backend::tir::lower_from_simple::lower_to_tir(&ir);
    let type_map = molt_backend::tir::type_refine::extract_type_map(&tir_func);
    let roundtripped = molt_backend::tir::lower_to_simple::lower_to_simple_ir(&tir_func, &type_map);

    // Check roundtripped transport still carries the compatibility hint.
    let rt_consts: Vec<_> = roundtripped
        .iter()
        .filter(|op| op.kind == "const" && op.fast_int == Some(true))
        .collect();
    assert!(
        !rt_consts.is_empty(),
        "roundtripped IR lost integer compatibility hints on const ops"
    );
}

#[test]
fn roundtrip_preserves_container_hint_on_list_new() {
    let ir = make_comprehension_ir();

    let orig_list_new: Vec<_> = ir
        .ops
        .iter()
        .filter(|op| op.kind == "list_new" && op.type_hint.as_deref() == Some("list"))
        .collect();
    assert_eq!(
        orig_list_new.len(),
        2,
        "original should have 2 list_new ops with list compatibility hints"
    );

    let tir_func = molt_backend::tir::lower_from_simple::lower_to_tir(&ir);
    let type_map = molt_backend::tir::type_refine::extract_type_map(&tir_func);
    let roundtripped = molt_backend::tir::lower_to_simple::lower_to_simple_ir(&tir_func, &type_map);

    let rt_list_new: Vec<_> = roundtripped
        .iter()
        .filter(|op| op.kind == "list_new" && op.type_hint.as_deref() == Some("list"))
        .collect();
    assert_eq!(
        rt_list_new.len(),
        2,
        "roundtripped lost list compatibility hint on list_new ops. Found: {:?}",
        roundtripped
            .iter()
            .filter(|op| op.kind == "list_new")
            .map(|op| &op.type_hint)
            .collect::<Vec<_>>()
    );
}

#[test]
fn roundtrip_preserves_container_hint_on_list_append() {
    let ir = make_comprehension_ir();

    let orig_append: Vec<_> = ir
        .ops
        .iter()
        .filter(|op| op.kind == "list_append" && op.type_hint.as_deref() == Some("list"))
        .collect();
    assert_eq!(
        orig_append.len(),
        1,
        "original should have list_append with list compatibility hint"
    );

    let tir_func = molt_backend::tir::lower_from_simple::lower_to_tir(&ir);
    let type_map = molt_backend::tir::type_refine::extract_type_map(&tir_func);
    let roundtripped = molt_backend::tir::lower_to_simple::lower_to_simple_ir(&tir_func, &type_map);

    let rt_append: Vec<_> = roundtripped
        .iter()
        .filter(|op| op.kind == "list_append")
        .collect();
    assert!(
        !rt_append.is_empty(),
        "roundtripped should have list_append"
    );
    assert_eq!(
        rt_append[0].type_hint.as_deref(),
        Some("list"),
        "list_append lost list compatibility hint after roundtrip. Got: {:?}",
        rt_append[0].type_hint
    );
}
