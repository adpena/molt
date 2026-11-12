use super::*;
use molt_backend::tir::{
    blocks::{Terminator, TirBlock},
    function::TirFunction,
    ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp},
    types::TirType,
    values::{TirValue, ValueId},
};

fn make_add_func() -> TirFunction {
    let mut func = TirFunction::new(
        "add".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    let v2 = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![v2],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![v2] };
    bind_arguments(func, &[AttrValue::Int(10), AttrValue::Int(32)])
}

fn make_const_func() -> TirFunction {
    let mut func = TirFunction::new(
        "const42".into(),
        vec![],
        TirType::I64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    let v0 = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let mut attrs = AttrDict::new();
    attrs.insert("value".into(), AttrValue::Int(42));
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ConstInt,
        operands: vec![],
        results: vec![v0],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![v0] };
    func
}

fn make_state_dispatch_func() -> TirFunction {
    let mut func = TirFunction::new(
        "generator_poll".into(),
        vec![TirType::I64],
        TirType::I64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    func.param_names = vec!["self".into()];
    let initial = func.fresh_block();
    let resumed = func.fresh_block();
    let initial_value = func.fresh_value();
    let resumed_value = func.fresh_value();
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::StateDispatch {
        cases: vec![(1, resumed, vec![])],
        default: initial,
        default_args: vec![],
    };
    for (block_id, value, constant) in [(initial, initial_value, 10), (resumed, resumed_value, 20)]
    {
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(constant));
        func.blocks.insert(
            block_id,
            TirBlock {
                id: block_id,
                args: vec![],
                ops: vec![TirOp {
                    origin: None,
                    runtime: None,
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![value],
                    attrs,
                    source_span: None,
                }],
                terminator: Terminator::Return {
                    values: vec![value],
                },
            },
        );
    }
    func
}

fn make_dynamic_not_func() -> TirFunction {
    let mut func = TirFunction::new(
        "dynamic_not".into(),
        vec![TirType::DynBox],
        TirType::Bool,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    func.value_types.insert(result, TirType::Bool);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Not,
        operands: vec![ValueId(0)],
        results: vec![result],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    func
}

fn make_original_kind_func() -> TirFunction {
    let mut func = TirFunction::new(
        "original_kind".into(),
        vec![],
        TirType::I64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    let result = func.fresh_value();
    func.value_types.insert(result, TirType::DynBox);
    let mut attrs = AttrDict::new();
    attrs.insert("semantic_id".into(), AttrValue::Int(7));
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: Some(molt_backend::OpKind::ModuleNew),
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Copy,
        operands: vec![],
        results: vec![result],
        attrs,
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![result],
    };
    func
}

fn make_typed_edge_func() -> TirFunction {
    let mut func = TirFunction::new(
        "typed_edge".into(),
        vec![],
        TirType::DynBox,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    let truth = func.fresh_value();
    let target_arg = func.fresh_value();
    let target = func.fresh_block();
    func.value_types.insert(truth, TirType::Bool);
    func.value_types.insert(target_arg, TirType::DynBox);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::ConstBool,
        operands: vec![],
        results: vec![truth],
        attrs: AttrDict::from([("value".into(), AttrValue::Bool(true))]),
        source_span: None,
    });
    entry.terminator = Terminator::Branch {
        target,
        args: vec![truth],
    };
    func.blocks.insert(
        target,
        TirBlock {
            id: target,
            args: vec![TirValue {
                id: target_arg,
                ty: TirType::DynBox,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![target_arg],
            },
        },
    );
    func
}

#[test]
fn test_rejects_async_work_poll_without_runtime_boundary() {
    let mut func = TirFunction::new(
        "async_work_poll".into(),
        vec![],
        TirType::None,
        molt_backend::ir::FunctionReturnAbi::Void,
    );
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    let mut poll = TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::CheckException,
        operands: vec![],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    };
    poll.mark_async_work_poll();
    entry.ops.push(poll);
    entry.terminator = Terminator::Return { values: vec![] };

    let ctx = create_mlir_context();
    let err = tir_to_mlir(&func, &ctx)
        .expect_err("MLIR must not erase the pending-call/eval-breaker poll");
    assert!(
        err.contains("async_work_poll"),
        "diagnostic must name the op: {err}"
    );
    assert!(
        err.contains(
            molt_backend::tir::op_kinds_generated::PENDING_CALL_EVAL_BREAKER_REQUIREMENT_REASON
        ),
        "diagnostic must name the missing target capability: {err}"
    );
}

#[test]
fn test_state_dispatch_refuses_unproven_runtime_receiver() {
    let ctx = create_mlir_context();
    let err = tir_to_mlir(&make_state_dispatch_func(), &ctx).unwrap_err();
    assert!(err.contains("parameter"), "{err}");
    assert!(err.contains("MLIR has no Molt runtime provider"), "{err}");
}

#[test]
fn test_dynamic_not_refuses_unproven_parameter() {
    let ctx = create_mlir_context();
    let err = tir_to_mlir(&make_dynamic_not_func(), &ctx).unwrap_err();
    assert!(err.contains("parameter"), "{err}");
    assert!(err.contains("MLIR has no Molt runtime provider"), "{err}");
}

#[test]
fn test_original_kind_refuses_boxed_return() {
    let ctx = create_mlir_context();
    let err = tir_to_mlir(&make_original_kind_func(), &ctx).unwrap_err();
    assert!(err.contains("return value"), "{err}");
    assert!(err.contains("MLIR has no Molt runtime provider"), "{err}");
}

#[test]
fn test_producer_carriers_flow_through_block_arguments() {
    let ctx = create_mlir_context();
    let module = tir_to_mlir(&make_typed_edge_func(), &ctx).unwrap();
    let text = module_to_string(&module);
    assert!(!text.contains("molt.box"));
    assert!(text.contains("i1"));
    assert!(text.contains("cf.br"));
    verify_module(&module).unwrap();
}

fn make_cond_func() -> TirFunction {
    make_cond_with_inputs(3, 5)
}

fn make_cond_with_inputs(a: i64, b: i64) -> TirFunction {
    let mut f = TirFunction::new(
        "cond".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    let cmp_val = f.fresh_value();
    let tb = f.fresh_block();
    let eb = f.fresh_block();
    let const_then = f.fresh_value();
    let const_else = f.fresh_value();

    // Entry: compare args, branch
    {
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(TirOp {
            origin: None,
            runtime: None,
            dialect: Dialect::Molt,
            opcode: OpCode::Lt,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![cmp_val],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::CondBranch {
            cond: cmp_val,
            then_block: tb,
            then_args: vec![],
            else_block: eb,
            else_args: vec![],
        };
    }

    // Then block: return 1
    let mut then_attrs = AttrDict::new();
    then_attrs.insert("value".into(), AttrValue::Int(1));
    f.blocks.insert(
        tb,
        TirBlock {
            id: tb,
            args: vec![],
            ops: vec![TirOp {
                origin: None,
                runtime: None,
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![const_then],
                attrs: then_attrs,
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![const_then],
            },
        },
    );

    // Else block: return 0
    let mut else_attrs = AttrDict::new();
    else_attrs.insert("value".into(), AttrValue::Int(0));
    f.blocks.insert(
        eb,
        TirBlock {
            id: eb,
            args: vec![],
            ops: vec![TirOp {
                origin: None,
                runtime: None,
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![const_else],
                attrs: else_attrs,
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![const_else],
            },
        },
    );
    bind_arguments(f, &[AttrValue::Int(a), AttrValue::Int(b)])
}

fn make_arith_chain_func() -> TirFunction {
    // f(a, b) = (a + b) * (a - b)
    let mut func = TirFunction::new(
        "arith_chain".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    let sum = func.fresh_value();
    let diff = func.fresh_value();
    let prod = func.fresh_value();
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Add,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![sum],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Sub,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![diff],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Mul,
        operands: vec![sum, diff],
        results: vec![prod],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![prod] };
    bind_arguments(func, &[AttrValue::Int(10), AttrValue::Int(3)])
}

fn make_pow_func() -> TirFunction {
    let mut func = TirFunction::new(
        "pow_i64".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    let out = func.fresh_value();
    func.value_types.insert(out, TirType::I64);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Pow,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![out],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![out] };
    func
}

fn make_malformed_pow_func() -> TirFunction {
    let mut func = TirFunction::new(
        "malformed_pow_i64".into(),
        vec![TirType::I64, TirType::I64],
        TirType::I64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::Pow,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return {
        values: vec![ValueId(0)],
    };
    func
}

fn make_float_floordiv_func() -> TirFunction {
    let mut func = TirFunction::new(
        "floordiv_f64".into(),
        vec![TirType::F64, TirType::F64],
        TirType::F64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    let out = func.fresh_value();
    func.value_types.insert(out, TirType::F64);
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::FloorDiv,
        operands: vec![ValueId(0), ValueId(1)],
        results: vec![out],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![out] };
    bind_arguments(func, &[AttrValue::Float(5.5), AttrValue::Float(2.0)])
}

fn make_unreachable_func() -> TirFunction {
    let mut func = TirFunction::new(
        "unreachable_i64".into(),
        vec![],
        TirType::I64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    let entry = func.blocks.get_mut(&func.entry_block).unwrap();
    entry.terminator = Terminator::Unreachable;
    func
}

#[test]
fn test_context_creation() {
    let _ctx = create_mlir_context();
}

#[test]
fn method_receiver_prefix_refuses_unproven_parameters() {
    let mut function = TirFunction::new(
        "method_receiver_prefix".into(),
        vec![TirType::DynBox, TirType::DynBox],
        TirType::None,
        molt_backend::ir::FunctionReturnAbi::Void,
    );
    let entry = function.blocks.get_mut(&function.entry_block).unwrap();
    entry.ops.push(TirOp {
        origin: None,
        runtime: None,
        dialect: Dialect::Molt,
        opcode: OpCode::CallargsPushMethodReceiver,
        operands: vec![entry.args[0].id, entry.args[1].id],
        results: vec![],
        attrs: AttrDict::new(),
        source_span: None,
    });
    entry.terminator = Terminator::Return { values: vec![] };
    let ctx = create_mlir_context();
    let err = tir_to_mlir(&function, &ctx).unwrap_err();
    assert!(err.contains("parameter %0"), "{err}");
    assert!(err.contains("MLIR has no Molt runtime provider"), "{err}");
}

#[test]
fn test_build_add_module() {
    let ctx = create_mlir_context();
    let module = tir_to_mlir(&make_add_func(), &ctx).unwrap();
    assert!(module.as_operation().verify());
    let text = module_to_string(&module);
    assert!(text.contains("arith.addi"));
    assert!(text.contains("func.func"));
}

#[test]
fn test_build_const_module() {
    let ctx = create_mlir_context();
    let module = tir_to_mlir(&make_const_func(), &ctx).unwrap();
    assert!(module.as_operation().verify());
    let text = module_to_string(&module);
    assert!(text.contains("arith.constant"));
    assert!(text.contains("42"));
}

#[test]
fn test_build_cond_module() {
    let ctx = create_mlir_context();
    let module = tir_to_mlir(&make_cond_func(), &ctx).unwrap();
    assert!(module.as_operation().verify());
    let text = module_to_string(&module);
    assert!(text.contains("arith.cmpi"));
    assert!(text.contains("cf.cond_br"));
}

#[test]
fn test_build_arith_chain_module() {
    let ctx = create_mlir_context();
    let module = tir_to_mlir(&make_arith_chain_func(), &ctx).unwrap();
    assert!(module.as_operation().verify());
    let text = module_to_string(&module);
    assert!(text.contains("arith.addi"));
    assert!(text.contains("arith.subi"));
    assert!(text.contains("arith.muli"));
}

#[test]
fn test_pow_refuses_annotation_only_parameters() {
    let ctx = create_mlir_context();
    let err = tir_to_mlir(&make_pow_func(), &ctx).unwrap_err();
    assert!(err.contains("parameter"), "{err}");
    assert!(err.contains("MLIR has no Molt runtime provider"), "{err}");
}

#[test]
fn test_malformed_pow_fails_closed_without_result_slot() {
    let ctx = create_mlir_context();
    let err = tir_to_mlir(&make_malformed_pow_func(), &ctx).unwrap_err();
    assert!(err.contains("malformed OpCode::Pow requires one result slot"));
}

#[test]
fn test_float_floordiv_uses_math_floor() {
    let ctx = create_mlir_context();
    let module = tir_to_mlir(&make_float_floordiv_func(), &ctx).unwrap();
    assert!(module.as_operation().verify());
    let text = module_to_string(&module);
    assert!(text.contains("arith.divf"));
    assert!(text.contains("math.floor"));
}

#[test]
fn test_unreachable_emits_fail_closed_assert() {
    let ctx = create_mlir_context();
    let module = tir_to_mlir(&make_unreachable_func(), &ctx).unwrap();
    assert!(module.as_operation().verify());
    let text = module_to_string(&module);
    assert!(text.contains("cf.assert"));
    assert!(text.contains("reached TIR unreachable terminator"));
}

#[test]
fn test_optimize_add() {
    let ctx = create_mlir_context();
    let mut module = tir_to_mlir(&make_add_func(), &ctx).unwrap();
    optimize::run_optimization_passes(&mut module, &ctx).unwrap();
    assert!(module.as_operation().verify());
}

#[test]
fn test_lower_to_llvm_add() {
    let ctx = create_mlir_context();
    let mut module = tir_to_mlir(&make_add_func(), &ctx).unwrap();
    lower::lower_to_llvm_dialect(&mut module, &ctx).unwrap();
    assert!(module.as_operation().verify());
    let text = module_to_string(&module);
    assert!(text.contains("llvm."));
}

#[test]
fn test_full_pipeline_add() {
    let result = compile_via_mlir(
        &make_add_func(),
        &MlirCompileOptions {
            opt_level: MlirOptLevel::O2,
            emit_llvm_dialect: true,
        },
    )
    .unwrap();
    assert!(result.llvm_dialect_text.contains("llvm."));
}

#[test]
fn test_full_pipeline_const() {
    let result = compile_via_mlir(
        &make_const_func(),
        &MlirCompileOptions {
            opt_level: MlirOptLevel::O2,
            emit_llvm_dialect: true,
        },
    )
    .unwrap();
    assert!(result.llvm_dialect_text.contains("llvm."));
}

#[test]
fn test_full_pipeline_cond() {
    let result = compile_via_mlir(
        &make_cond_func(),
        &MlirCompileOptions {
            opt_level: MlirOptLevel::O2,
            emit_llvm_dialect: true,
        },
    )
    .unwrap();
    assert!(result.llvm_dialect_text.contains("llvm."));
}

#[test]
fn test_full_pipeline_state_dispatch_refuses_unproven_receiver() {
    let err =
        compile_via_mlir(&make_state_dispatch_func(), &MlirCompileOptions::default()).unwrap_err();
    assert!(err.contains("generator_poll parameter %0"), "{err}");
    assert!(err.contains("MLIR has no Molt runtime provider"), "{err}");
}

#[test]
fn test_full_pipeline_arith_chain() {
    let result = compile_via_mlir(
        &make_arith_chain_func(),
        &MlirCompileOptions {
            opt_level: MlirOptLevel::O2,
            emit_llvm_dialect: true,
        },
    )
    .unwrap();
    assert!(result.llvm_dialect_text.contains("llvm."));
}

#[test]
fn test_jit_add() {
    let result = jit_execute_i64(&make_add_func(), "add", &[]).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn test_jit_const() {
    let result = jit_execute_i64(&make_const_func(), "const42", &[]).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn test_jit_cond_true() {
    // 3 < 5 => return 1
    let result = jit_execute_i64(&make_cond_func(), "cond", &[]).unwrap();
    assert_eq!(result, 1);
}

#[test]
fn test_jit_cond_false() {
    // 7 < 2 => false => return 0
    let result = jit_execute_i64(&make_cond_with_inputs(7, 2), "cond", &[]).unwrap();
    assert_eq!(result, 0);
}

#[test]
fn test_jit_arith_chain() {
    // (10 + 3) * (10 - 3) = 13 * 7 = 91
    let result = jit_execute_i64(&make_arith_chain_func(), "arith_chain", &[]).unwrap();
    assert_eq!(result, 91);
}

#[test]
fn test_optimized_module_creation() {
    let ctx = create_mlir_context();
    let result = create_optimized_module(
        &make_add_func(),
        &ctx,
        &MlirCompileOptions {
            opt_level: MlirOptLevel::O2,
            emit_llvm_dialect: false,
        },
    )
    .unwrap();
    assert!(!result.standard_mlir_text.is_empty());
    assert!(!result.optimized_mlir_text.is_empty());
}

// Closing a synthetic callable gives each block parameter an executable producer,
// rather than blessing its declaration as a raw ABI. JIT tests then take no args.
pub(crate) fn bind_arguments(mut func: TirFunction, values: &[AttrValue]) -> TirFunction {
    assert_eq!(values.len(), func.param_types.len());
    // The range solver does not yet transfer arbitrary non-loop integer phi
    // bounds into arithmetic. Keep integer operation fixtures on literal SSA
    // producers; the float/bool fixtures below exercise proven block parameters.
    if values
        .iter()
        .all(|value| matches!(value, AttrValue::Int(_)))
    {
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let constants = entry
            .args
            .drain(..)
            .zip(values)
            .map(|(arg, value)| TirOp {
                origin: None,
                runtime: None,
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![arg.id],
                attrs: AttrDict::from([("value".into(), value.clone())]),
                source_span: None,
            })
            .collect::<Vec<_>>();
        entry.ops.splice(0..0, constants);
        func.param_types.clear();
        func.param_names.clear();
        return func;
    }
    let target = func.entry_block;
    let entry_id = func.fresh_block();
    let mut args = Vec::new();
    let mut ops = Vec::new();
    for value in values {
        let id = func.fresh_value();
        let (opcode, ty) = match value {
            AttrValue::Int(_) => (OpCode::ConstInt, TirType::I64),
            AttrValue::Float(_) => (OpCode::ConstFloat, TirType::F64),
            AttrValue::Bool(_) => (OpCode::ConstBool, TirType::Bool),
            _ => panic!("not a scalar test input"),
        };
        func.value_types.insert(id, ty);
        args.push(id);
        ops.push(TirOp {
            origin: None,
            runtime: None,
            dialect: Dialect::Molt,
            opcode,
            operands: vec![],
            results: vec![id],
            attrs: AttrDict::from([("value".into(), value.clone())]),
            source_span: None,
        });
    }
    func.blocks.insert(
        entry_id,
        TirBlock {
            id: entry_id,
            args: vec![],
            ops,
            terminator: Terminator::Branch { target, args },
        },
    );
    func.entry_block = entry_id;
    func.param_types.clear();
    func.param_names.clear();
    func
}

#[test]
fn annotation_only_float_parameter_is_refused_but_producer_parameter_is_admitted() {
    let mut func = TirFunction::new(
        "float_identity".into(),
        vec![TirType::F64],
        TirType::F64,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Return {
        values: vec![ValueId(0)],
    };
    let ctx = create_mlir_context();
    let err = tir_to_mlir(&func, &ctx).unwrap_err();
    assert!(err.contains("float_identity parameter %0"), "{err}");
    assert!(err.contains("annotation-only"), "{err}");
    let proven = bind_arguments(func, &[AttrValue::Float(1.5)]);
    let module = tir_to_mlir(&proven, &ctx).expect("constant producer proves block parameter");
    verify_module(&module).unwrap();
    let text = module_to_string(&module);
    assert!(text.contains("f64"), "{text}");
    assert!(!text.contains("molt.box"), "{text}");
}

#[test]
fn unproven_block_parameter_is_refused_before_emission() {
    let mut func = make_original_kind_func();
    let incoming = match &func.blocks[&func.entry_block].terminator {
        Terminator::Return { values } => values[0],
        _ => panic!("fixture must return its opaque producer"),
    };
    let target = func.fresh_block();
    let argument = func.fresh_value();
    func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::Branch {
        target,
        args: vec![incoming],
    };
    func.blocks.insert(
        target,
        TirBlock {
            id: target,
            args: vec![TirValue {
                id: argument,
                ty: TirType::F64,
            }],
            ops: vec![],
            terminator: Terminator::Return {
                values: vec![argument],
            },
        },
    );
    let ctx = create_mlir_context();
    let err = tir_to_mlir(&func, &ctx).unwrap_err();
    assert!(
        err.contains(&format!("{} block argument %{}", func.name, argument.0)),
        "{err}"
    );
    assert!(err.contains("annotation-only"), "{err}");
}

#[test]
fn implicit_boxed_return_requires_a_runtime_provider() {
    let mut func = TirFunction::new(
        "implicit_none".into(),
        vec![],
        TirType::None,
        molt_backend::ir::FunctionReturnAbi::Value,
    );
    func.blocks.get_mut(&func.entry_block).unwrap().terminator =
        Terminator::Return { values: vec![] };
    let ctx = create_mlir_context();
    let err = tir_to_mlir(&func, &ctx).unwrap_err();
    assert!(
        err.contains("implicit_none return value <implicit None>"),
        "{err}"
    );
    assert!(err.contains("MLIR has no Molt runtime provider"), "{err}");
}

#[test]
fn unused_annotation_only_scalar_operations_cannot_bypass_boundary_admission() {
    for opcode in [
        OpCode::Neg,
        OpCode::Pos,
        OpCode::Bool,
        OpCode::Not,
        OpCode::Add,
        OpCode::Eq,
    ] {
        let mut func = make_original_kind_func();
        let source = ValueId(0);
        func.value_types.insert(source, TirType::F64);
        let unused = func.fresh_value();
        let returned = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            origin: None,
            runtime: None,
            dialect: Dialect::Molt,
            opcode,
            operands: if matches!(opcode, OpCode::Add | OpCode::Eq) {
                vec![source, source]
            } else {
                vec![source]
            },
            results: vec![unused],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            origin: None,
            runtime: None,
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![returned],
            attrs: AttrDict::from([("value".into(), AttrValue::Int(42))]),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![returned],
        };
        let ctx = create_mlir_context();
        let err = tir_to_mlir(&func, &ctx).unwrap_err();
        assert!(
            err.contains("original_kind")
                && err.contains("value %0")
                && err.contains("annotation-only"),
            "{opcode:?}: {err}"
        );
    }
}
