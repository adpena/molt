use super::super::*;

#[test]
fn membership_uses_contains_runtime_call_and_not_in_inverts_bool() {
    let cases = [
        ("in_dynbox", OpCode::In, false),
        ("not_in_dynbox", OpCode::NotIn, true),
    ];

    for (name, opcode, expect_inversion) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::Bool,
        );
        let result_id = func.fresh_value();
        func.value_types.insert(result_id, TirType::Bool);
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
            output.runtime_calls.contains(&"contains"),
            "{name} must call contains(container, item); got {:?}",
            output.runtime_calls
        );
        assert_eq!(
            output
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, Instruction::I32Eqz)),
            expect_inversion,
            "{name} must invert only for NotIn"
        );
    }
}
