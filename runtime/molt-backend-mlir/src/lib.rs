//! MLIR progressive lowering pipeline for TIR.
//!
//! Implements the Mojo-inspired progressive lowering:
//! TIR -> Standard MLIR (func/arith/cf/scf) -> LLVM dialect -> JIT/object code.
//!
//! This crate uses melior (Rust MLIR bindings) to:
//! 1. Build MLIR modules programmatically from TIR using standard dialect ops
//! 2. Run MLIR optimization passes (canonicalize, CSE, LICM, SCCP, etc.)
//! 3. Lower through SCF->CF->LLVM dialect conversion
//! 4. Emit LLVM IR text or JIT-execute via the MLIR execution engine
//!
//! The crate intentionally lives outside `molt-backend` so the MLIR/LLVM stack
//! can evolve independently of the inkwell/LLVM stack used by the LLVM backend.

mod lower;
mod optimize;
mod pipeline;
mod tir_to_mlir;

pub use pipeline::{
    MlirCompileOptions, MlirCompileResult, MlirOptLevel, compile_via_mlir,
    create_optimized_module, jit_execute_i64,
};

use melior::{
    Context as MlirContext,
    dialect::DialectRegistry,
    ir::{Location, Module as MlirModule, operation::OperationLike},
    utility::{register_all_dialects, register_all_llvm_translations},
};

/// Create an MLIR context with all standard dialects and LLVM translations registered.
pub fn create_mlir_context() -> MlirContext {
    let ctx = MlirContext::new();
    let registry = DialectRegistry::new();
    register_all_dialects(&registry);
    ctx.append_dialect_registry(&registry);
    ctx.load_all_available_dialects();
    register_all_llvm_translations(&ctx);
    ctx
}

/// Convert a TIR function to an MLIR module using the programmatic builder API.
///
/// This builds real MLIR ops (arith.constant, arith.addi, cf.br, func.func, etc.)
/// from TIR ops, producing a verifiable MLIR module in standard dialects.
pub fn tir_to_mlir<'c>(
    func: &molt_backend::tir::function::TirFunction,
    ctx: &'c MlirContext,
) -> Result<MlirModule<'c>, String> {
    tir_to_mlir::build_mlir_module(func, ctx)
}

/// Run MLIR verification on a module.
pub fn verify_module(module: &MlirModule<'_>) -> Result<(), String> {
    if module.as_operation().verify() {
        Ok(())
    } else {
        Err("MLIR module verification failed".to_string())
    }
}

/// Get the textual MLIR IR representation of a module.
pub fn module_to_string(module: &MlirModule<'_>) -> String {
    module.as_operation().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_backend::tir::{
        blocks::{BlockId, Terminator, TirBlock},
        function::TirFunction,
        ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp},
        types::TirType,
        values::ValueId,
    };

    fn make_add_func() -> TirFunction {
        let mut func =
            TirFunction::new("add".into(), vec![TirType::I64, TirType::I64], TirType::I64);
        let v2 = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![v2],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![v2] };
        func
    }

    fn make_const_func() -> TirFunction {
        let mut func = TirFunction::new("const42".into(), vec![], TirType::I64);
        let v0 = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        let mut attrs = AttrDict::new();
        attrs.insert("value".into(), AttrValue::Int(42));
        entry.ops.push(TirOp {
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

    fn make_cond_func() -> TirFunction {
        let mut f = TirFunction::new("cond".into(), vec![TirType::I64, TirType::I64], TirType::I64);
        let cmp_val = f.fresh_value();
        let tb = f.fresh_block();
        let eb = f.fresh_block();
        let const_then = f.fresh_value();
        let const_else = f.fresh_value();

        // Entry: compare args, branch
        {
            let entry = f.blocks.get_mut(&f.entry_block).unwrap();
            entry.ops.push(TirOp {
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
        f
    }

    fn make_arith_chain_func() -> TirFunction {
        // f(a, b) = (a + b) * (a - b)
        let mut func = TirFunction::new(
            "arith_chain".into(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
        );
        let sum = func.fresh_value();
        let diff = func.fresh_value();
        let prod = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![sum],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Sub,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![diff],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Mul,
            operands: vec![sum, diff],
            results: vec![prod],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![prod] };
        func
    }

    #[test]
    fn test_context_creation() {
        let _ctx = create_mlir_context();
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
        let result = jit_execute_i64(&make_add_func(), "add", &[10, 32]).unwrap();
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
        let result = jit_execute_i64(&make_cond_func(), "cond", &[3, 5]).unwrap();
        assert_eq!(result, 1);
    }

    #[test]
    fn test_jit_cond_false() {
        // 7 < 2 => false => return 0
        let result = jit_execute_i64(&make_cond_func(), "cond", &[7, 2]).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_jit_arith_chain() {
        // (10 + 3) * (10 - 3) = 13 * 7 = 91
        let result = jit_execute_i64(&make_arith_chain_func(), "arith_chain", &[10, 3]).unwrap();
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
}
