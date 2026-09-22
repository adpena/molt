use super::super::*;

#[test]
#[should_panic(expected = "compiler arena placement has no proved owner lifetime")]
fn arena_eligible_alloc_is_rejected_without_owner_lifetime() {
    let mut func = TirFunction::new(
        "arena_alloc".into(),
        vec![],
        TirType::DynBox,
        molt_ir::FunctionReturnAbi::Value,
    );
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Alloc,
        operands: vec![],
        results: vec![result_id],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("value".into(), AttrValue::Int(32));
            m.insert("arena_eligible".into(), AttrValue::Bool(true));
            m
        },
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let _ = lower_tir_to_wasm(&func);
}
