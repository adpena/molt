use super::super::*;

fn unary_carrier_function(opcode: OpCode, operand_ty: TirType) -> (TirFunction, ValueId) {
    let mut func = TirFunction::new("unary_carrier".into(), vec![operand_ty], TirType::DynBox);
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![ValueId(0)],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    (func, result)
}

#[test]
fn unary_numeric_boxed_results_materialize_operand_carriers() {
    for (opcode, runtime_call) in [(OpCode::Neg, "neg"), (OpCode::Pos, "pos")] {
        for (operand_ty, operand_repr) in [
            (TirType::I64, Repr::RawI64FullDeopt),
            (TirType::F64, Repr::FloatUnboxed),
            (TirType::Bool, Repr::Bool),
            (TirType::DynBox, Repr::MaybeBigInt),
        ] {
            let (func, result) = unary_carrier_function(opcode, operand_ty);
            let repr = HashMap::from([(ValueId(0), operand_repr), (result, Repr::MaybeBigInt)]);
            let vr = crate::representation_plan::value_range_for(&func);
            let lir = lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
            let output = lower_lir_to_wasm(&lir).test_view();
            assert!(!output.bails_to_generic_path, "{opcode:?}/{operand_repr:?}");
            assert!(
                output.runtime_calls.contains(&runtime_call),
                "{opcode:?}/{operand_repr:?}"
            );
            assert!(
                !output
                    .instructions
                    .iter()
                    .any(|i| matches!(i, Instruction::I64Sub | Instruction::F64Neg))
            );
            match operand_repr {
                Repr::RawI64FullDeopt => assert!(output.runtime_calls.contains(&"int_from_i64")),
                Repr::Bool => assert!(output.instructions.iter().any(|i| matches!(i, Instruction::I64Const(bits) if *bits == molt_codegen_abi::QNAN_TAG_BOOL_I64))),
                Repr::FloatUnboxed => assert!(output.instructions.iter().any(|i| matches!(i, Instruction::I64ReinterpretF64))),
                _ => {}
            }
        }
    }
}

#[test]
fn unary_runtime_results_do_not_enter_raw_numeric_locals() {
    for opcode in [OpCode::Neg, OpCode::Pos] {
        for result_repr in [Repr::RawI64Safe, Repr::FloatUnboxed] {
            let (func, result) = unary_carrier_function(opcode, TirType::DynBox);
            let repr = HashMap::from([(ValueId(0), Repr::MaybeBigInt), (result, result_repr)]);
            let vr = crate::representation_plan::value_range_for(&func);
            let lir = lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
            let output = lower_lir_to_wasm(&lir).test_view();
            assert!(
                output.bails_to_generic_path,
                "boxed {opcode:?} result must not masquerade as {result_repr:?}"
            );
        }
    }
}

#[test]
fn unary_neg_requires_lir_overflow_proof_even_for_raw_result_carriers() {
    let (func, result) = unary_carrier_function(OpCode::Neg, TirType::I64);
    for result_repr in [Repr::RawI64Safe, Repr::RawI64FullDeopt] {
        let repr = HashMap::from([(ValueId(0), Repr::RawI64FullDeopt), (result, result_repr)]);
        let vr = crate::representation_plan::value_range_for(&func);
        let lir = lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
        let unary = &lir.blocks[&lir.entry_block].ops[0];
        assert_eq!(
            unary.tir_op.attrs.get("lir.boxed_dispatch"),
            Some(&AttrValue::Bool(true))
        );
        let output = lower_lir_to_wasm(&lir).test_view();
        assert!(output.runtime_calls.contains(&"neg"));
        assert!(output.runtime_calls.contains(&"int_from_i64"));
        assert!(
            !output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::I64Sub))
        );
        assert!(output.bails_to_generic_path);
    }
}

#[test]
fn unary_proven_numeric_results_keep_raw_operations_and_float_signs() {
    let mut func = make_const_return_func(42);
    let operand = ValueId(0);
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Neg,
        operands: vec![operand],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    let output = lower_tir_to_wasm(&func).test_view();
    assert!(!output.bails_to_generic_path);
    assert!(!output.runtime_calls.contains(&"neg"));
    assert!(
        output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::I64Sub))
    );

    for opcode in [OpCode::Neg, OpCode::Pos] {
        let (func, result) = unary_carrier_function(opcode, TirType::F64);
        let repr = HashMap::from([
            (ValueId(0), Repr::FloatUnboxed),
            (result, Repr::FloatUnboxed),
        ]);
        let vr = crate::representation_plan::value_range_for(&func);
        let lir = lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
        let output = lower_lir_to_wasm(&lir).test_view();
        assert!(!output.bails_to_generic_path);
        assert!(
            !output
                .runtime_calls
                .iter()
                .any(|call| matches!(*call, "neg" | "pos"))
        );
        // IEEE f64.neg flips the sign bit (including +/-0); unary plus must
        // leave all bits untouched. A subtraction-from-zero would lose -0.
        assert_eq!(
            output
                .instructions
                .iter()
                .filter(|i| matches!(i, Instruction::F64Neg))
                .count(),
            usize::from(opcode == OpCode::Neg)
        );
        assert!(
            !output
                .instructions
                .iter()
                .any(|i| matches!(i, Instruction::F64Sub))
        );
    }
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
fn bool_bitwise_results_stay_bool1_without_boxed_runtime_calls() {
    let cases = [
        ("bit_and_bool", OpCode::BitAnd),
        ("bit_or_bool", OpCode::BitOr),
        ("bit_xor_bool", OpCode::BitXor),
    ];

    for (name, opcode) in cases {
        let mut func = TirFunction::new(name.into(), vec![], TirType::Bool);
        let lhs = func.fresh_value();
        let rhs = func.fresh_value();
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        for (id, value) in [(lhs, true), (rhs, false)] {
            entry.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstBool,
                operands: vec![],
                results: vec![id],
                attrs: AttrDict::from([("value".into(), AttrValue::Bool(value))]),
                source_span: None,
            });
        }
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![lhs, rhs],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        let output = lower_tir_to_wasm(&func).test_view();
        assert!(
            !output.bails_to_generic_path,
            "{name} must stay in the LIR fast lane"
        );
        assert!(
            !output
                .runtime_calls
                .iter()
                .any(|call| matches!(*call, "bit_and" | "bit_or" | "bit_xor")),
            "{name} must not feed a boxed i64 result into its Bool1 local: {:?}",
            output.runtime_calls
        );
        assert!(
            output.instructions.iter().any(|instruction| match opcode {
                OpCode::BitAnd => matches!(instruction, Instruction::I32And),
                OpCode::BitOr => matches!(instruction, Instruction::I32Or),
                OpCode::BitXor => matches!(instruction, Instruction::I32Xor),
                _ => unreachable!(),
            }),
            "{name} must use the typed Bool1 bitwise instruction: {:?}",
            output.instructions
        );
    }
}

#[test]
fn raw_bitwise_operands_with_boxed_results_use_boxed_runtime_calls() {
    let cases = [
        ("bit_and_raw_to_boxed", OpCode::BitAnd, "bit_and"),
        ("bit_or_raw_to_boxed", OpCode::BitOr, "bit_or"),
        ("bit_xor_raw_to_boxed", OpCode::BitXor, "bit_xor"),
    ];

    for (name, opcode, runtime_call) in cases {
        let mut func =
            TirFunction::new(name.into(), vec![TirType::I64, TirType::I64], TirType::I64);
        let result = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };
        let repr = HashMap::from([
            (ValueId(0), Repr::RawI64FullDeopt),
            (ValueId(1), Repr::RawI64FullDeopt),
            (result, Repr::MaybeBigInt),
        ]);
        let vr = crate::representation_plan::value_range_for(&func);
        let lir = lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
        let output = lower_lir_to_wasm(&lir).test_view();

        assert!(
            !output.bails_to_generic_path,
            "{name} must keep its boxed result in the LIR fast lane"
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{name} must not store a raw machine result in a DynBox local: {:?}",
            output.runtime_calls
        );
        assert!(
            output
                .runtime_calls
                .iter()
                .filter(|call| **call == "int_from_i64")
                .count()
                >= 2,
            "{name} must box both full-width raw operands before runtime dispatch: {:?}",
            output.runtime_calls
        );
    }
}

#[test]
fn integer_invert_with_raw_input_and_boxed_result_uses_boxed_runtime() {
    let mut func = TirFunction::new(
        "invert_raw_carrier".into(),
        vec![TirType::I64],
        TirType::I64,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BitNot,
        operands: vec![ValueId(0)],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    let vr = crate::representation_plan::value_range_for(&func);

    let boxed_result_repr = HashMap::from([
        (ValueId(0), Repr::RawI64FullDeopt),
        (result, Repr::MaybeBigInt),
    ]);
    let boxed_lir = lower_function_to_lir_with_inline_proof(&func, &boxed_result_repr, &vr);
    let boxed_output = lower_lir_to_wasm(&boxed_lir).test_view();
    assert!(
        boxed_output.runtime_calls.contains(&"invert"),
        "full-width raw invert with a boxed result must use the boxed runtime: {:?}",
        boxed_output.runtime_calls
    );
    assert!(
        boxed_output.runtime_calls.contains(&"int_from_i64"),
        "full-width raw invert input must be boxed overflow-safely: {:?}",
        boxed_output.runtime_calls
    );
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
