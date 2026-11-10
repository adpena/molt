use super::*;
use molt_backend::tir::{
    blocks::Terminator,
    function::TirFunction,
    ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp},
    types::TirType,
    values::ValueId,
};

fn make_identity_func() -> TirFunction {
    let mut func = TirFunction::new(
        "identity".into(),
        vec![TirType::I64],
        TirType::I64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::Return {
        values: vec![ValueId(0)],
    };
    crate::tests::bind_arguments(func, &[AttrValue::Int(99)])
}

fn make_sub_func() -> TirFunction {
    let mut func = TirFunction::new(
        "sub".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Sub,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    crate::tests::bind_arguments(func, &[AttrValue::Int(100), AttrValue::Int(58)])
}

#[test]
fn test_pipeline_identity() {
    let result = jit_execute_i64(&make_identity_func(), "identity", &[]).unwrap();
    assert_eq!(result, 99);
}

#[test]
fn test_pipeline_sub() {
    let result = jit_execute_i64(&make_sub_func(), "sub", &[]).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn test_compile_result_stages() {
    let func = TirFunction::new(
        "empty_ret".into(),
        vec![],
        TirType::I64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    // The unreachable terminator emits an assert trap before the verifier-required return.
    let result = compile_via_mlir(
        &func,
        &MlirCompileOptions {
            opt_level: MlirOptLevel::O1,
            emit_llvm_dialect: true,
        },
    )
    .unwrap();

    // All stages should produce non-empty text.
    assert!(!result.standard_mlir_text.is_empty());
    assert!(!result.optimized_mlir_text.is_empty());
    assert!(!result.llvm_dialect_text.is_empty());

    // Standard should have func.func, LLVM should have llvm.
    assert!(result.standard_mlir_text.contains("func.func"));
    assert!(result.llvm_dialect_text.contains("llvm."));
}

#[test]
fn test_default_options() {
    let opts = MlirCompileOptions::default();
    assert_eq!(opts.opt_level, MlirOptLevel::O2);
    assert!(opts.emit_llvm_dialect);
}
