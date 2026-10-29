use super::super::*;

#[test]
fn operand_name_attrs_stay_lir_fast_runtime_calls() {
    let cases = [
        (
            molt_ir::OpKind::GetAttrName,
            OpCode::LoadAttr,
            2,
            true,
            "get_attr_name",
        ),
        (
            molt_ir::OpKind::SetAttrName,
            OpCode::StoreAttr,
            3,
            false,
            "set_attr_name",
        ),
        (
            molt_ir::OpKind::DelAttrName,
            OpCode::DelAttr,
            2,
            false,
            "del_attr_name",
        ),
    ];

    for (kind, opcode, operand_count, has_result, runtime_call) in cases {
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
            attrs: AttrDict::new(),
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
