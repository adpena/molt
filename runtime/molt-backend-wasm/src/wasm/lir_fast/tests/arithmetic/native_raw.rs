use super::super::*;

#[test]
fn add_two_i64s() {
    let func = make_add_two_consts_func(20, 22);

    let output = lower_tir_to_wasm(&func).test_view();

    assert_eq!(output.param_types, Vec::<ValType>::new());

    // Should contain i64.add.
    let has_add = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::I64Add));
    assert!(has_add, "expected i64.add instruction");
}

#[test]
fn bool1_and_stays_raw_without_selected_ref_retain() {
    let mut func = TirFunction::new(
        "and_bool1".into(),
        vec![TirType::Bool, TirType::Bool],
        TirType::Bool,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::And,
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
        output
            .instructions
            .iter()
            .any(|i| matches!(i, Instruction::I32And)),
        "raw Bool1 and must stay a native i32.and: {:?}",
        output.instructions
    );
    assert!(
        !output.runtime_calls.contains(&"inc_ref_obj"),
        "raw Bool1 and must not retain a selected boxed operand: {:?}",
        output.runtime_calls
    );
}

#[test]
fn raw_unary_pos_stays_noop_without_runtime_call() {
    let mut func = TirFunction::new("pos_raw_i64".into(), vec![TirType::I64], TirType::I64);
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Pos,
        operands: vec![ValueId(0)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let repr = HashMap::from([
        (ValueId(0), Repr::RawI64Safe),
        (result_id, Repr::RawI64Safe),
    ]);
    let vr = crate::representation_plan::value_range_for(&func);
    let lir = lower_function_to_lir_with_inline_proof(&func, &repr, &vr);
    let output = lower_lir_to_wasm(&lir).test_view();

    assert!(
        !output.bails_to_generic_path,
        "proven raw unary plus must stay in the LIR fast lane"
    );
    assert_eq!(output.param_types, vec![ValType::I64]);
    assert_eq!(output.result_types, vec![ValType::I64]);
    assert!(
        !output.runtime_calls.contains(&"pos"),
        "proven raw unary plus must remain a no-op, not call pos: {:?}",
        output.runtime_calls
    );
}

#[test]
fn add_two_f64s() {
    let mut func = TirFunction::new(
        "add_f64".into(),
        vec![TirType::F64, TirType::F64],
        TirType::F64,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert_eq!(output.param_types, vec![ValType::F64, ValType::F64]);
    let has_f64_add = output
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::F64Add));
    assert!(has_f64_add, "expected f64.add instruction");
}

#[test]
fn f64_mod_declares_emission_scratch_locals() {
    let mut func = TirFunction::new(
        "mod_f64".into(),
        vec![TirType::F64, TirType::F64],
        TirType::F64,
    );
    let result_id = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        dialect: Dialect::Molt,
        opcode: OpCode::Mod,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result_id],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result_id],
    };

    let output = lower_tir_to_wasm(&func).test_view();

    assert_eq!(output.param_types, vec![ValType::F64, ValType::F64]);
    assert_eq!(output.result_types, vec![ValType::F64]);
    assert_eq!(
        output.locals,
        vec![ValType::F64, ValType::F64, ValType::F64],
        "f64 modulo needs the result local plus two scratch locals declared"
    );
}
