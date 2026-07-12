use super::super::*;

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
