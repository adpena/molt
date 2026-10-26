use super::*;

#[test]
fn normalized_boundary_rejects_pre_ssa_transport_with_source_identity() {
    for kind in ["loop_index_start", "loop_index_next"] {
        let carrier = OpIR {
            kind: kind.into(),
            args: Some(vec!["seed".into()]),
            out: Some("index".into()),
            source_op_idx: Some(73),
            ..OpIR::default()
        };
        // This is legal authored transport; only the normalized boundary rejects it.
        validate_simple_op_shape(&carrier).expect("frontend carrier remains admitted");
        let error = validate_normalized_simple_ir_ops("counted", &[carrier]).unwrap_err();
        assert_eq!(error.function, "counted");
        assert_eq!(error.op_index, 0);
        assert_eq!(error.shape.kind, kind);
        assert_eq!(
            error.shape.violation,
            OpShapeViolation::Unnormalized {
                source_op_idx: Some(73)
            }
        );
        let message = error.to_string();
        assert!(message.contains("function `counted` op#0"), "{message}");
        assert!(
            message.contains(kind) && message.contains("source op#73"),
            "{message}"
        );
    }
}

#[test]
fn normalized_boundary_keeps_executable_structured_and_value_operations() {
    let ops: Vec<_> = [
        "store_var",
        "loop_start",
        "load_var",
        "loop_break_if_false",
        "store_var",
        "loop_continue",
        "loop_end",
        "ret_void",
    ]
    .into_iter()
    .map(|kind| OpIR {
        kind: kind.into(),
        ..OpIR::default()
    })
    .collect();
    // The boundary is a read-only normal-form check, not a second shape or CFG validator.
    validate_normalized_simple_ir_ops("normalized", &ops).unwrap();
    validate_normalized_simple_ir_ops("empty", &[]).unwrap();
}
