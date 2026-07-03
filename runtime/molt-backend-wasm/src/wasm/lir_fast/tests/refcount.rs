use super::*;

#[test]
fn lir_fast_lane_dec_ref_emits_named_runtime_call() {
    let mut func = TirFunction::new("drop_ref".into(), vec![], TirType::None);
    let owned = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstNone,
        operands: vec![],
        results: vec![owned],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DecRef,
        operands: vec![owned],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let output = lower_tir_to_wasm(&func).test_view();
    assert!(
        output.runtime_calls.contains(&"dec_ref_obj"),
        "WASM LIR fast lane must consume shared DecRef through dec_ref_obj; got {:?}",
        output.runtime_calls
    );
}

#[test]
fn lir_fast_lane_del_boundary_emits_named_dec_ref_runtime_call() {
    let mut func = TirFunction::new("del_boundary_release".into(), vec![], TirType::None);
    let owned = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::ConstNone,
        operands: vec![],
        results: vec![owned],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DelBoundary,
        operands: vec![owned],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };

    let output = lower_tir_to_wasm(&func).test_view();
    assert!(
        output.runtime_calls.contains(&"dec_ref_obj"),
        "WASM LIR fast lane must consume DelBoundary through dec_ref_obj; got {:?}",
        output.runtime_calls
    );
}
