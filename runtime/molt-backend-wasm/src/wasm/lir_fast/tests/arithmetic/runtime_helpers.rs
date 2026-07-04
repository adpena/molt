use super::super::*;

#[test]
fn dynbox_or_retains_selected_operand_result() {
    let mut func = TirFunction::new(
        "or_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Or,
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
        output.runtime_calls.contains(&"is_truthy"),
        "boxed or must test Python truthiness: {:?}",
        output.runtime_calls
    );
    assert!(
        output.runtime_calls.contains(&"inc_ref_obj"),
        "boxed or must retain the selected borrowed operand result: {:?}",
        output.runtime_calls
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::LocalTee(_))),
        "boxed or must tee the selected result before retaining it: {:?}",
        output.instructions
    );
}

#[test]
fn dynbox_unary_scalar_helpers_stay_lir_fast_runtime_calls() {
    let cases = [
        ("neg_dynbox", OpCode::Neg, "neg"),
        ("pos_dynbox", OpCode::Pos, "pos"),
        ("invert_dynbox", OpCode::BitNot, "invert"),
    ];

    for (name, opcode, runtime_call) in cases {
        let mut func = TirFunction::new(name.into(), vec![TirType::DynBox], TirType::DynBox);
        let result_id = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![ValueId(0)],
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
            output.runtime_calls.contains(&runtime_call),
            "{name} must call {runtime_call}; got {:?}",
            output.runtime_calls
        );
    }
}

#[test]
fn dynbox_pow_stays_lir_fast_runtime_call() {
    let mut func = TirFunction::new(
        "pow_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Pow,
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
        "DynBox pow must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"pow"),
        "DynBox pow must dispatch through the typed runtime helper; got {:?}",
        output.runtime_calls
    );
}

#[test]
fn dynbox_binary_bitwise_and_shift_helpers_stay_lir_fast_runtime_calls() {
    let cases = [
        ("bit_and_dynbox", OpCode::BitAnd, "bit_and"),
        ("bit_or_dynbox", OpCode::BitOr, "bit_or"),
        ("bit_xor_dynbox", OpCode::BitXor, "bit_xor"),
        ("lshift_dynbox", OpCode::Shl, "lshift"),
        ("rshift_dynbox", OpCode::Shr, "rshift"),
    ];

    for (name, opcode, runtime_call) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
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
            output.runtime_calls.contains(&runtime_call),
            "{name} must call {runtime_call}; got {:?}",
            output.runtime_calls
        );
    }
}

#[test]
fn dynbox_inplace_arithmetic_uses_generated_numeric_lir_helpers() {
    let cases = [
        (
            "inplace_add_dynbox",
            OpCode::InplaceAdd,
            "inplace_add",
            "add",
        ),
        (
            "inplace_sub_dynbox",
            OpCode::InplaceSub,
            "inplace_sub",
            "sub",
        ),
        (
            "inplace_mul_dynbox",
            OpCode::InplaceMul,
            "inplace_mul",
            "mul",
        ),
    ];

    for (name, opcode, expected_runtime_call, rejected_runtime_call) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
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
            output.runtime_calls.contains(&expected_runtime_call),
            "{name} must call {expected_runtime_call}; got {:?}",
            output.runtime_calls
        );
        assert!(
            !output.runtime_calls.contains(&rejected_runtime_call),
            "{name} must not collapse to {rejected_runtime_call}; got {:?}",
            output.runtime_calls
        );
    }
}
