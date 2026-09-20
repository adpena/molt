use super::super::*;

#[test]
fn heap_alloc_stays_lir_fast_through_immediate_runtime_call() {
    let mut func = TirFunction::new("heap_alloc".into(), vec![], TirType::DynBox);
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
        "ordinary heap Alloc must stay in the LIR fast lane"
    );
    assert!(
        output.runtime_calls.contains(&"alloc"),
        "heap Alloc must call alloc; got {:?}",
        output.runtime_calls
    );
    assert_eq!(
        output.runtime_calls,
        ["alloc", "object_publish_initialized"],
        "allocation must publish before its owned result can escape"
    );
    assert!(
        output
            .instructions
            .iter()
            .any(|instruction| matches!(instruction, Instruction::I64Const(32))),
        "heap Alloc must pass its size attr as an immediate"
    );
}

#[test]
fn released_heap_alloc_publishes_before_releasing_its_owner() {
    let mut func = TirFunction::new("released_heap_alloc".into(), vec![], TirType::None);
    let result = func.fresh_value();
    func.value_types.insert(result, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Alloc,
        operands: vec![],
        results: vec![result],
        attrs: AttrDict::from([("value".into(), AttrValue::Int(32))]),
        source_span: None,
    });
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::DecRef,
        operands: vec![result],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };
    crate::tir::verify::verify_function(&func).unwrap();
    let output = lower_tir_to_wasm(&func).test_view();
    assert!(!output.bails_to_generic_path);
    assert_eq!(
        output.runtime_calls,
        ["alloc", "object_publish_initialized", "dec_ref_obj"]
    );
}
