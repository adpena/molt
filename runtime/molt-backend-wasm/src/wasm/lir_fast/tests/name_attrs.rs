use super::*;

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

#[test]
fn literal_name_attrs_without_site_id_stay_lir_fast_runtime_calls() {
    let cases = [
        (
            "get_attr_generic_ptr",
            OpCode::LoadAttr,
            1,
            true,
            vec!["handle_resolve", "get_attr_ptr"],
        ),
        (
            "get_attr_special_obj",
            OpCode::LoadAttr,
            1,
            true,
            vec!["get_attr_special"],
        ),
        (
            "set_attr_generic_ptr",
            OpCode::StoreAttr,
            2,
            false,
            vec!["set_attr_object"],
        ),
        (
            "set_attr_generic_obj",
            OpCode::StoreAttr,
            2,
            false,
            vec!["set_attr_object"],
        ),
        (
            "del_attr_generic_ptr",
            OpCode::DelAttr,
            1,
            false,
            vec!["del_attr_object"],
        ),
        (
            "del_attr_generic_obj",
            OpCode::DelAttr,
            1,
            false,
            vec!["del_attr_object"],
        ),
    ];

    for (name, opcode, operand_count, has_result, runtime_calls) in cases {
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

#[test]
fn generic_obj_literal_name_attr_uses_source_site_ic_id() {
    let func_name = "generic_obj_literal_name_attr";
    let source_op_idx = 23usize;
    let mut func = TirFunction::new(func_name.into(), vec![TirType::DynBox], TirType::DynBox);
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::LoadAttr,
        operands: vec![ValueId(0)],
        results: vec![result_id],
        attrs: {
            let mut m = AttrDict::new();
            m.insert(
                "_original_kind".into(),
                AttrValue::Str("get_attr_generic_obj".into()),
            );
            m.insert("name".into(), AttrValue::Str("field".into()));
            m.insert(
                "_source_op_idx".into(),
                AttrValue::Int(source_op_idx as i64),
            );
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        !output.bails_to_generic_path,
        "get_attr_generic_obj must stay in the LIR fast lane when source op identity is carried"
    );
    assert!(
        output.runtime_calls.contains(&"get_attr_object_ic"),
        "get_attr_generic_obj must call get_attr_object_ic; got {:?}",
        output.runtime_calls
    );
    assert!(
        output.data_ptr_i32_literals.contains(&b"field".to_vec()),
        "get_attr_generic_obj must carry the literal name as a LIR data pointer; got {:?}",
        output.data_ptr_i32_literals
    );
    let expected_site_bits = box_int_bits(stable_ic_site_id(
        func_name,
        source_op_idx,
        "get_attr_generic_obj",
    ));
    assert!(
        output
            .instructions
            .iter()
            .any(|inst| matches!(inst, Instruction::I64Const(bits) if *bits == expected_site_bits)),
        "get_attr_generic_obj must use source op identity for the IC site id"
    );
}

#[test]
#[should_panic(expected = "get_attr_generic_obj requires source op index")]
fn generic_obj_literal_name_attr_without_source_op_index_fails_closed() {
    let mut func = TirFunction::new(
        "get_attr_generic_obj_without_source".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::LoadAttr,
        operands: vec![ValueId(0)],
        results: vec![result_id],
        attrs: {
            let mut m = AttrDict::new();
            m.insert(
                "_original_kind".into(),
                AttrValue::Str("get_attr_generic_obj".into()),
            );
            m.insert("name".into(), AttrValue::Str("field".into()));
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let _ = lower_tir_to_wasm(&func);
}
