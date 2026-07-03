use super::*;

#[test]
fn add_two_i64s() {
    let func = make_add_two_consts_func(20, 22);

    let output = lower_tir_to_wasm(&func).test_view();

    assert_eq!(output.param_types, Vec::<ValType>::new());

    // Should contain i64.add.
    let has_add = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::I64Add));
    assert!(has_add, "expected i64.add instruction");
}

#[test]
fn bool1_and_stays_raw_without_selected_ref_retain() {
    let mut func = TirFunction::new(
        "and_bool1".into(),
        vec![TirType::Bool, TirType::Bool],
        TirType::Bool,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::And,
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
        output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::I32And)),
        "raw Bool1 and must stay a native i32.and: {:?}",
        output.instructions
    );
    assert!(
        !output.runtime_calls.contains(&"inc_ref_obj"),
        "raw Bool1 and must not retain a selected boxed operand: {:?}",
        output.runtime_calls
    );
}

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
fn raw_unary_pos_stays_noop_without_runtime_call() {
    let mut func = TirFunction::new("pos_raw_i64".into(), vec![TirType::I64], TirType::I64);
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Pos,
        operands: vec![ValueId(0)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let repr = HashMap::from([
        (ValueId(0), Repr::RawI64Safe),
        (result_id, Repr::RawI64Safe),
    ]);
    let vr = crate::representation_plan::value_range_for(&func);
    let lir = lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
    let output = lower_lir_to_wasm(&lir).test_view();

    assert!(
        !output.bails_to_generic_path,
        "proven raw unary plus must stay in the LIR fast lane"
    );
    assert_eq!(output.param_types, vec![ValType::I64]);
    assert_eq!(output.result_types, vec![ValType::I64]);
    assert!(
        !output.runtime_calls.contains(&"pos"),
        "proven raw unary plus must remain a no-op, not call pos: {:?}",
        output.runtime_calls
    );
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

#[test]
fn preserved_copy_numeric_helpers_use_generated_fixed_runtime_selector() {
    let cases = [
        ("inplace_div", 2, "inplace_div"),
        ("inplace_floordiv", 2, "inplace_floordiv"),
        ("inplace_mod", 2, "inplace_mod"),
        ("inplace_pow", 2, "inplace_pow"),
        ("matmul", 2, "matmul"),
        ("inplace_matmul", 2, "inplace_matmul"),
        ("pow_mod", 3, "pow_mod"),
        ("round", 3, "round"),
        ("trunc", 1, "trunc"),
        ("string_eq", 2, "string_eq"),
        ("shl", 2, "lshift"),
        ("shr", 2, "rshift"),
        ("bit_not", 1, "invert"),
        ("unary_neg", 1, "neg"),
        ("unary_pos", 1, "pos"),
    ];

    for (original_kind, operand_count, runtime_call) in cases {
        let func =
            make_copy_original_kind_runtime_func(original_kind, original_kind, operand_count, true);
        let output = lower_tir_to_wasm(&func).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{original_kind} must stay in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{original_kind} must call {runtime_call}; got {:?}",
            output.runtime_calls
        );
    }
}

#[test]
fn add_two_f64s() {
    let mut func = TirFunction::new(
        "add_f64".into(),
        vec![TirType::F64, TirType::F64],
        TirType::F64,
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

    assert_eq!(output.param_types, vec![ValType::F64, ValType::F64]);
    let has_f64_add = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::F64Add));
    assert!(has_f64_add, "expected f64.add instruction");
}

#[test]
fn f64_mod_declares_emission_scratch_locals() {
    let mut func = TirFunction::new(
        "mod_f64".into(),
        vec![TirType::F64, TirType::F64],
        TirType::F64,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Mod,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert_eq!(output.param_types, vec![ValType::F64, ValType::F64]);
    assert_eq!(output.result_types, vec![ValType::F64]);
    assert_eq!(
        output.locals,
        vec![ValType::F64, ValType::F64, ValType::F64],
        "f64 modulo needs the result local plus two scratch locals declared"
    );
}
