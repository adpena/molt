use super::super::*;

#[test]
fn dynbox_index_store_and_delete_stay_lir_fast_runtime_calls() {
    let mut index_func = TirFunction::new(
        "index_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let index_result = index_func.fresh_value();
    index_func.value_types.insert(index_result, TirType::DynBox);
    let entry = index_func.blocks.get_mut(&index_func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Index,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![index_result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![index_result],
    };

    let output = lower_tir_to_wasm(&index_func).test_view();
    assert!(
        !output.bails_to_generic_path,
        "DynBox index must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"index"),
        "DynBox index must dispatch through the boxed index helper; got {:?}",
        output.runtime_calls
    );

    let mut store_func = TirFunction::new(
        "store_index_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let entry = store_func.blocks.get_mut(&store_func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::StoreIndex,
        operands: vec![ValueId(0), ValueId(1), ValueId(2)],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![ValueId(0)],
    };

    let output = lower_tir_to_wasm(&store_func).test_view();
    assert!(
        !output.bails_to_generic_path,
        "DynBox store_index must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"store_index"),
        "DynBox store_index must dispatch through the boxed store helper; got {:?}",
        output.runtime_calls
    );

    let mut del_func = TirFunction::new(
        "del_index_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let entry = del_func.blocks.get_mut(&del_func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DelIndex,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![ValueId(0)],
    };

    let output = lower_tir_to_wasm(&del_func).test_view();
    assert!(
        !output.bails_to_generic_path,
        "DynBox del_index must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"del_index"),
        "DynBox del_index must dispatch through the boxed delete helper; got {:?}",
        output.runtime_calls
    );
}
