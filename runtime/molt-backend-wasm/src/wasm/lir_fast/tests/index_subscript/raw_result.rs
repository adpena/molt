use super::super::*;

#[test]
fn raw_index_result_refuses_boxed_runtime_bits() {
    let mut func = TirFunction::new(
        "raw_index_result".into(),
        vec![TirType::DynBox, TirType::I64],
        TirType::I64,
    );
    let result_id = func.fresh_value();
    func.value_types.insert(result_id, TirType::I64);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Index,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let repr = HashMap::from([
        (ValueId(0), Repr::DynBox),
        (ValueId(1), Repr::RawI64Safe),
        (result_id, Repr::RawI64Safe),
    ]);
    let vr = crate::representation_plan::value_range_for(&func);
    let lir = lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
    let output = lower_lir_to_wasm(&lir).test_view();

    assert!(
        output.bails_to_generic_path,
        "raw index result must not store boxed runtime bits into an I64 carrier"
    );
    assert_eq!(
        output.bail_to_generic_reason,
        Some(WasmLirFallbackReason::UnsupportedOperation)
    );
}
