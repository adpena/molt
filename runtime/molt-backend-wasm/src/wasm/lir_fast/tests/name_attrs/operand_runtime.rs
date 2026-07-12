use super::super::*;

#[test]
fn operand_name_attrs_stay_lir_fast_runtime_calls() {
    let cases = [
        ("get_attr_name", OpCode::LoadAttr, 2, true, "get_attr_name"),
        (
            "set_attr_name",
            OpCode::StoreAttr,
            3,
            false,
            "set_attr_name",
        ),
        ("del_attr_name", OpCode::DelAttr, 2, false, "del_attr_name"),
    ];

    for (name, opcode, operand_count, has_result, runtime_call) in cases {
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox; operand_count],
            if has_result {
                TirType::DynBox
            } else {
                TirType::None
            },
        );
        let result_id = has_result.then(|| {
            let id = func.fresh_value();
            func.value_types.insert(id, TirType::DynBox);
            id
        });
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
            results: result_id.into_iter().collect(),
            attrs: {
                let mut m = AttrDict::new();
                m.insert("_original_kind".into(), AttrValue::Str(name.into()));
                m
            },
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: result_id.into_iter().collect(),
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
