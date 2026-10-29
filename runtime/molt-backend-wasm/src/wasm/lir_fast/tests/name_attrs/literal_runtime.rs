use super::super::*;

#[test]
fn literal_name_attrs_without_site_id_stay_lir_fast_runtime_calls() {
    let cases = [
        (
            molt_ir::OpKind::GetAttrSpecialObj,
            OpCode::LoadAttr,
            1,
            true,
            vec!["get_attr_special"],
        ),
        (
            molt_ir::OpKind::DelAttrGenericPtr,
            OpCode::DelAttr,
            1,
            false,
            vec!["del_attr_object"],
        ),
        (
            molt_ir::OpKind::DelAttrGenericObj,
            OpCode::DelAttr,
            1,
            false,
            vec!["del_attr_object"],
        ),
    ];

    for (kind, opcode, operand_count, has_result, runtime_calls) in cases {
        let name = kind.as_str();
        let mut func = TirFunction::new(
            name.into(),
            vec![TirType::DynBox; operand_count],
            if has_result {
                TirType::DynBox
            } else {
                TirType::None
            },
            molt_ir::FunctionReturnAbi::Value,
        );
        let result_id = has_result.then(|| {
            let id = func.fresh_value();
            func.value_types.insert(id, TirType::DynBox);
            id
        });
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            origin: Some(kind),
            dialect: Dialect::Molt,
            opcode,
            operands: (0..operand_count).map(|idx| ValueId(idx as u32)).collect(),
            results: result_id.into_iter().collect(),
            attrs: {
                let mut m = AttrDict::new();

                m.insert("name".into(), AttrValue::Str("field".into()));
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
            output.data_ptr_i32_literals.contains(&b"field".to_vec()),
            "{name} must carry the literal name as a LIR data pointer; got {:?}",
            output.data_ptr_i32_literals
        );
        for runtime_call in runtime_calls {
            assert!(
                output.runtime_calls.contains(&runtime_call),
                "{name} must call {runtime_call}; got {:?}",
                output.runtime_calls
            );
        }
    }
}
