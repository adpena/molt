use super::super::*;

#[test]
fn arena_eligible_alloc_stays_explicit_generic_fallback() {
    let mut func = TirFunction::new("arena_alloc".into(), vec![], TirType::DynBox);
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

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        output.bails_to_generic_path,
        "arena-eligible Alloc must not be heap-lowered until LIR owns arena locals"
    );
    assert_eq!(
        output.bail_to_generic_reason,
        Some(WasmLirFallbackReason::UnsupportedOperation)
    );
    assert!(
        !output.runtime_calls.contains(&"alloc"),
        "arena-eligible Alloc must not silently fall back to heap alloc"
    );
}
