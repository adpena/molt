use super::*;

#[test]
fn metadata_only_structured_loop_ops_skips_unmatched_loop_controls() {
    let ops = vec![
        op_kind("state_switch"),
        op_kind("loop_start"),
        OpIR {
            kind: "loop_break_if_true".to_string(),
            args: Some(vec!["done".to_string()]),
            ..OpIR::default()
        },
        OpIR {
            kind: "label".to_string(),
            value: Some(365),
            ..OpIR::default()
        },
        OpIR {
            kind: "br_if".to_string(),
            args: Some(vec!["done".to_string()]),
            value: Some(343),
            ..OpIR::default()
        },
    ];

    assert_eq!(
        metadata_only_structured_loop_ops(&ops),
        BTreeSet::from([1usize, 2usize]),
        "TIR-linearized label loops must not also lower stale structured loop markers",
    );
}

#[test]
fn metadata_only_structured_loop_ops_preserves_matched_nested_loops() {
    let ops = vec![
        op_kind("loop_start"),
        op_kind("loop_break_if_false"),
        op_kind("loop_start"),
        op_kind("loop_continue"),
        op_kind("loop_end"),
        op_kind("loop_break"),
        op_kind("loop_end"),
    ];

    assert!(
        metadata_only_structured_loop_ops(&ops).is_empty(),
        "well-formed structured loop ranges remain executable native CFG",
    );
}
