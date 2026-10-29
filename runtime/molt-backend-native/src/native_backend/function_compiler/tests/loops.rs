use super::*;
use molt_ir::OpKind;

#[test]
fn metadata_only_structured_loop_ops_skips_unmatched_loop_controls() {
    let ops = vec![
        op_kind(OpKind::StateSwitch),
        op_kind(OpKind::LoopStart),
        OpIR {
            args: Some(vec!["done".to_string()]),
            ..OpIR::new(OpKind::LoopBreakIfTrue)
        },
        OpIR {
            value: Some(365),
            ..OpIR::new(OpKind::Label)
        },
        OpIR {
            args: Some(vec!["done".to_string()]),
            value: Some(343),
            ..OpIR::new(OpKind::BrIf)
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
        op_kind(OpKind::LoopStart),
        op_kind(OpKind::LoopBreakIfFalse),
        op_kind(OpKind::LoopStart),
        op_kind(OpKind::LoopContinue),
        op_kind(OpKind::LoopEnd),
        op_kind(OpKind::LoopBreak),
        op_kind(OpKind::LoopEnd),
    ];

    assert!(
        metadata_only_structured_loop_ops(&ops).is_empty(),
        "well-formed structured loop ranges remain executable native CFG",
    );
}
