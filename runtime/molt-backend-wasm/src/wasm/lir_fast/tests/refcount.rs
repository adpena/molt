use super::*;

#[test]
fn lir_fast_lane_dec_ref_emits_named_runtime_call() {
    let mut func = TirFunction::new(
        "drop_ref".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let owned = func.blocks[&func.entry_block].args[0].id;
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
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
    let mut func = TirFunction::new(
        "del_boundary_release".into(),
        vec![TirType::DynBox],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let owned = func.blocks[&func.entry_block].args[0].id;
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
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

#[test]
fn producer_proven_none_has_no_refcount_instructions_or_admission() {
    let mut func = TirFunction::new(
        "immortal_rc".into(),
        vec![],
        TirType::None,
        molt_ir::FunctionReturnAbi::Void,
    );
    let owned = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ConstNone,
        operands: vec![],
        results: vec![owned],
        attrs: AttrDict::new(),
        source_span: None,
    });
    for opcode in [OpCode::IncRef, OpCode::DecRef] {
        entry.ops.push(TirOp {
            origin: None,
            dialect: Dialect::Molt,
            opcode,
            operands: vec![owned],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
    }
    entry.terminator = Terminator::Return { values: vec![] };
    let output = lower_tir_to_wasm(&func).test_view();
    assert!(
        output.runtime_calls.is_empty(),
        "proven None has no RC or invocation-admission operation: {:?}",
        output.runtime_calls
    );
    assert!(!output.instructions.iter().any(|inst| matches!(
        inst,
        Instruction::I32Load(_) | Instruction::I32Store(_) | Instruction::I64Load32U(_)
    )));
}

#[test]
fn boxed_proven_scalars_have_no_refcount_or_fallible_boxing_boundary() {
    for (opcode, value) in [
        (OpCode::ConstInt, crate::tir::ops::AttrValue::Int(7)),
        (OpCode::ConstBool, crate::tir::ops::AttrValue::Bool(true)),
        (OpCode::ConstFloat, crate::tir::ops::AttrValue::Float(1.25)),
    ] {
        let mut func = TirFunction::new(
            "boxed_immediate_rc".into(),
            vec![],
            TirType::None,
            molt_ir::FunctionReturnAbi::Void,
        );
        let source = func.fresh_value();
        let boxed = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            origin: None,
            dialect: Dialect::Molt,
            opcode,
            operands: vec![],
            results: vec![source],
            attrs: AttrDict::from([(
                if opcode == OpCode::ConstFloat {
                    "f_value"
                } else {
                    "value"
                }
                .into(),
                value,
            )]),
            source_span: None,
        });
        entry.ops.push(TirOp {
            origin: None,
            dialect: Dialect::Molt,
            opcode: OpCode::BoxVal,
            operands: vec![source],
            results: vec![boxed],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            origin: None,
            dialect: Dialect::Molt,
            opcode: OpCode::DecRef,
            operands: vec![boxed],
            results: vec![],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![] };
        let output = lower_tir_to_wasm(&func).test_view();
        assert!(
            output.runtime_calls.is_empty(),
            "{opcode:?}: boxed proven scalar needs no RC, admission, fallible allocation or temporary-owner cleanup: {:?}",
            output.runtime_calls
        );
    }
}
