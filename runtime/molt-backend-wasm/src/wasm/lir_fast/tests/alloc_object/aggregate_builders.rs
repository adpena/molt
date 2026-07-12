use super::super::*;

#[test]
fn aggregate_builders_stay_lir_fast_runtime_calls() {
    let cases = [
        (
            "build_list",
            OpCode::BuildList,
            3,
            vec![
                ("list_builder_new", 1),
                ("list_builder_append", 3),
                ("list_builder_finish", 1),
            ],
        ),
        (
            "build_tuple",
            OpCode::BuildTuple,
            2,
            vec![
                ("list_builder_new", 1),
                ("list_builder_append", 2),
                ("tuple_builder_finish", 1),
            ],
        ),
        (
            "build_dict",
            OpCode::BuildDict,
            4,
            vec![("dict_new", 1), ("dict_set", 2)],
        ),
        (
            "build_set",
            OpCode::BuildSet,
            3,
            vec![("set_new", 1), ("set_add", 3)],
        ),
    ];

    for (name, opcode, operand_count, expected_calls) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox; operand_count],
            TirType::DynBox,
        );
        let result_id = func.fresh_value();
        func.value_types.insert(result_id, TirType::DynBox);
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
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
        for (runtime_call, expected_count) in expected_calls {
            let actual_count = output
                .runtime_calls
                .iter()
                .filter(|&&call| call == runtime_call)
                .count();
            assert_eq!(
                actual_count, expected_count,
                "{name} runtime call {runtime_call} count mismatch: {:?}",
                output.runtime_calls
            );
        }
    }
}
