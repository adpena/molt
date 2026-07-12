use super::super::*;

#[test]
fn alloc_task_bails_to_generic_emission() {
    let mut func = TirFunction::new("alloc_task".into(), vec![TirType::DynBox], TirType::DynBox);
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::AllocTask,
        operands: vec![ValueId(0)],
        results: vec![result_id],
        attrs: {
            let mut m = AttrDict::new();
            m.insert("s_value".into(), AttrValue::Str("task_poll".into()));
            m.insert("task_kind".into(), AttrValue::Str("future".into()));
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
        "alloc_task must bail to generic WASM emission"
    );
    assert_eq!(
        output.bail_to_generic_reason,
        Some(WasmLirFallbackReason::UnsupportedOperation)
    );
}

#[test]
fn state_switch_bails_to_generic_emission() {
    let mut func = TirFunction::new(
        "state_switch".into(),
        vec![TirType::DynBox],
        TirType::DynBox,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::StateSwitch,
        operands: vec![ValueId(0)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert!(
        output.bails_to_generic_path,
        "state_switch must bail to generic WASM emission"
    );
    assert_eq!(
        output.bail_to_generic_reason,
        Some(WasmLirFallbackReason::UnsupportedOperation)
    );
}
