use super::super::*;

fn make_boxed_checked_binary_func(name: &str, opcode: OpCode) -> TirFunction {
    let mut func = TirFunction::new(
        name.into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    let overflow_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    func.value_types.insert(overflow_id, TirType::Bool);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id, overflow_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };
    func
}

#[test]
fn checked_overflow_triples_use_opcode_specific_runtime_helpers_without_generic_bail() {
    let cases = [
        ("checked_add_overflow_triple", OpCode::Add, 20, 22, "add"),
        ("checked_sub_overflow_triple", OpCode::Sub, 42, 20, "sub"),
        ("checked_mul_overflow_triple", OpCode::Mul, 6, 7, "mul"),
    ];

    for (name, opcode, lhs, rhs, runtime_call) in cases {
        let func = make_binary_two_consts_func(name, opcode, lhs, rhs);
        let output = lower_tir_to_wasm(&func).test_view();

        assert_eq!(
            output.bail_to_generic_reason, None,
            "{name} must stay in the LIR fast body"
        );
        assert!(
            has_native_binary_instruction(&output.instructions, opcode),
            "{name} must emit the hot raw WASM instruction for {opcode:?}"
        );
        assert!(
            output.runtime_calls.contains(&runtime_call),
            "{name} overflow side channel must dispatch through {runtime_call}; got {:?}",
            output.runtime_calls
        );
        for other in ["add", "sub", "mul"] {
            if other != runtime_call {
                assert!(
                    !output.runtime_calls.contains(&other),
                    "{name} must not call the {other} helper for a {opcode:?} overflow side channel; got {:?}",
                    output.runtime_calls
                );
            }
        }
    }
}

#[test]
fn boxed_checked_arithmetic_uses_one_explicit_fallback_family() {
    for (name, opcode) in [
        ("boxed_checked_add", OpCode::CheckedAdd),
        ("boxed_checked_mul", OpCode::CheckedMul),
    ] {
        let func = make_boxed_checked_binary_func(name, opcode);
        let output = lower_tir_to_wasm(&func).test_view();

        assert_eq!(
            output.bail_to_generic_reason,
            Some(WasmLirFallbackReason::BoxedCheckedArithmetic),
            "{name} must use the checked-arithmetic fallback authority"
        );
    }
}

/// The perf-preservation direction: when BOTH operands are proven
/// `RawI64Safe`, the fast `i64.add` is still emitted (the checked-overflow
/// triple). The cold overflow-box side channel is a typed runtime call, not a
/// generic body bail.
#[test]
fn proven_raw_i64_add_still_emits_native_i64_add() {
    let func = make_add_two_consts_func(20, 22);
    let output = lower_tir_to_wasm(&func).test_view();

    let has_operand_add = output.instructions.iter().enumerate().any(|(idx, inst)| {
        matches!(inst, Instruction::I64Add)
            && !matches!(
                output.instructions.get(idx + 1),
                Some(Instruction::I64Const(c)) if *c == (1i64 << 47)
            )
    });
    assert!(
        has_operand_add,
        "range-proven const add must emit an operand-pair native i64.add, got {:?}",
        output.instructions
    );
    assert_eq!(output.bail_to_generic_reason, None);
    assert!(
        output.runtime_calls.contains(&"add"),
        "checked-overflow cold side channel must use the typed add helper"
    );
}

#[test]
fn checked_mul_raw_i64_emits_exact_wasm_overflow_flag_without_generic_bail() {
    let func = make_checked_mul_two_consts_func(6, 7);
    let output = lower_tir_to_wasm(&func).test_view();

    assert_eq!(
        output.bail_to_generic_reason, None,
        "raw checked_mul must stay in the LIR fast body"
    );
    assert!(
        !output.runtime_calls.contains(&"mul"),
        "raw CheckedMul produces a raw carrier and must not route through boxed molt_mul"
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64Mul)),
        "raw CheckedMul must emit the wrapping i64.mul product"
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64DivS)),
        "raw CheckedMul must emit the exact product/lhs overflow check"
    );
    assert!(
        output.instructions.iter().any(
            |instruction| matches!(instruction, Instruction::I64Const(value) if *value == i64::MIN)
        ),
        "raw CheckedMul must guard the i64::MIN / -1 division trap"
    );
}
