use super::super::*;

#[test]
#[should_panic(expected = "get_attr_generic_obj requires source op index")]
fn generic_obj_literal_name_attr_without_source_op_index_fails_closed() {
    lower_without_source_index("get_attr_generic_obj");
}

#[test]
#[should_panic(expected = "get_attr_generic_ptr requires source op index")]
fn generic_ptr_literal_name_attr_without_source_op_index_fails_closed() {
    lower_without_source_index("get_attr_generic_ptr");
}

fn lower_without_source_index(kind: &str) {
    let mut func = TirFunction::new(
        format!("{kind}_without_source"),
        vec![TirType::DynBox],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
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
            m.insert("_original_kind".into(), AttrValue::Str(kind.into()));
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
