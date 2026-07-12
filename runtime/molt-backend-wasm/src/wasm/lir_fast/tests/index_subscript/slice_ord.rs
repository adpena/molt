use super::super::*;

#[test]
fn build_slice_stays_lir_fast_and_pads_missing_bounds_with_none() {
    let mut func = TirFunction::new(
        "build_slice_missing_step".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::BuildSlice,
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
        "BuildSlice must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"slice_new"),
        "BuildSlice must call the slice_new ABI helper; got {:?}",
        output.runtime_calls
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64Const(bits) if *bits == box_none_bits())),
        "BuildSlice must pad a missing step operand with the ABI None bits"
    );
}

#[test]
fn ord_at_stays_lir_fast_with_boxed_result_carrier() {
    let mut func = TirFunction::new(
        "ord_at_dynbox".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::OrdAt,
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
        "OrdAt must stay in the LIR fast lane after boxed carrier authority is fixed"
    );
    assert!(
        output.runtime_calls.contains(&"ord_at"),
        "OrdAt must call the ord_at ABI helper; got {:?}",
        output.runtime_calls
    );
}
