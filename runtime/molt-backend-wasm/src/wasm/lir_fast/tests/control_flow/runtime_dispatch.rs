use super::super::*;

#[test]
fn dynbox_add_falls_back_to_call() {
    let mut func = TirFunction::new(
        "add_dyn".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        output.runtime_calls.contains(&"add"),
        "expected typed runtime import for DynBox add"
    );

    let has_i64_add = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::I64Add));
    assert!(!has_i64_add, "should NOT emit i64.add for DynBox operands");
}

#[test]
fn mixed_f64_dynbox_add_boxes_float_without_generic_bail() {
    let mut func = TirFunction::new(
        "add_float_dyn".into(),
        vec![TirType::F64, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        !output.bails_to_generic_path,
        "F64 operand boxing must not poison typed LIR-fast runtime dispatch with a generic bail"
    );
    assert!(
        output.runtime_calls.contains(&"add"),
        "mixed F64/DynBox add must dispatch through the typed boxed runtime helper"
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64ReinterpretF64)),
        "F64 operand must be boxed by reinterpreting its IEEE payload"
    );
    assert!(
        output.instructions.iter().any(
            |instruction| matches!(instruction, Instruction::I64Const(mask) if *mask == F64_EXPONENT_MASK)
        ),
        "F64 boxing must use the shared all-NaN exponent mask"
    );
    assert!(
        output.instructions.iter().any(
            |instruction| matches!(instruction, Instruction::I64Const(mask) if *mask == F64_FRACTION_MASK)
        ),
        "F64 boxing must use the shared all-NaN fraction mask"
    );
    assert!(
        output.instructions.iter().any(
            |instruction| matches!(instruction, Instruction::I64Const(bits) if *bits == CANONICAL_NAN_BITS as i64)
        ),
        "F64 boxing must canonicalize NaN payloads to the shared canonical bits"
    );
}

#[test]
fn dynbox_identity_comparisons_stay_lir_fast_runtime_calls() {
    let cases = [
        ("is_dynbox", OpCode::Is, false),
        ("is_not_dynbox", OpCode::IsNot, true),
    ];

    for (name, opcode, expect_invert) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::Bool,
        );
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };

        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&"is"),
            "{name} must dispatch through the identity helper; got {:?}",
            output.runtime_calls
        );
        assert!(
            !output.runtime_calls.contains(&"not"),
            "{name} should project/invert the boxed bool locally for Bool1 results"
        );
        assert_eq!(
            output
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::I32Eqz)),
            expect_invert,
            "{name} local Bool1 projection invert mismatch: {:?}",
            output.instructions
        );
    }
}
