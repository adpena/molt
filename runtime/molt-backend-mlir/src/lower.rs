//! MLIR lowering to LLVM dialect.
//!
//! Implements the progressive lowering from standard MLIR dialects to the LLVM
//! dialect using MLIR's built-in conversion passes:
//!
//! 1. `convert-scf-to-cf` -- Structured control flow -> basic block control flow
//! 2. `convert-to-llvm` -- All standard dialects -> LLVM dialect (arith, func, cf, memref, index)
//! 3. `reconcile-unrealized-casts` -- Clean up any remaining type conversions
//!
//! After lowering, the module is entirely in the LLVM dialect and can be:
//! - Printed as LLVM dialect text
//! - Fed to the MLIR execution engine for JIT compilation
//! - Translated to LLVM IR (via the execution engine internals)

use melior::{
    Context as MlirContext,
    ir::Module as MlirModule,
    pass::{self, PassManager},
};

/// Lower a module from standard dialects (func/arith/cf/scf) to the LLVM dialect.
///
/// This is a one-way transformation. After calling this, the module will contain
/// only LLVM dialect operations and cannot be meaningfully optimized at the
/// MLIR level (LLVM's own pass manager should be used instead, e.g., via the
/// execution engine's optimization level parameter).
pub fn lower_to_llvm_dialect(
    module: &mut MlirModule<'_>,
    ctx: &MlirContext,
) -> Result<(), String> {
    let pm = PassManager::new(ctx);
    pm.enable_verifier(true);

    // Step 1: Lower structured control flow (scf.for, scf.while, scf.if) to
    // basic block control flow (cf.br, cf.cond_br). This must happen before
    // the CF->LLVM conversion.
    pm.add_pass(pass::conversion::create_scf_to_control_flow());

    // Step 2: Use the all-in-one convert-to-llvm pass which handles:
    // - arith -> llvm
    // - func -> llvm
    // - cf -> llvm
    // - memref -> llvm
    // - index -> llvm
    // This is equivalent to running individual conversion passes but handles
    // cross-dialect dependencies correctly.
    pm.add_pass(pass::conversion::create_to_llvm());

    // Step 3: Reconcile any remaining unrealized_conversion_cast operations
    // that may be left over from partial conversions.
    pm.add_pass(pass::conversion::create_reconcile_unrealized_casts());

    pm.run(module)
        .map_err(|e| format!("MLIR -> LLVM dialect lowering failed: {e}"))
}

/// Lower with explicit individual conversion passes instead of create_to_llvm.
///
/// This gives finer control over the lowering order and is useful when
/// debugging which specific conversion step fails.
pub fn lower_to_llvm_dialect_stepwise(
    module: &mut MlirModule<'_>,
    ctx: &MlirContext,
) -> Result<(), String> {
    // Step 1: SCF -> CF
    {
        let pm = PassManager::new(ctx);
        pm.enable_verifier(true);
        pm.add_pass(pass::conversion::create_scf_to_control_flow());
        pm.run(module)
            .map_err(|e| format!("SCF->CF lowering failed: {e}"))?;
    }

    // Step 2: Arith -> LLVM
    {
        let pm = PassManager::new(ctx);
        pm.enable_verifier(true);
        pm.add_pass(pass::conversion::create_arith_to_llvm());
        pm.run(module)
            .map_err(|e| format!("Arith->LLVM lowering failed: {e}"))?;
    }

    // Step 3: Index -> LLVM
    {
        let pm = PassManager::new(ctx);
        pm.enable_verifier(true);
        pm.add_pass(pass::conversion::create_index_to_llvm());
        pm.run(module)
            .map_err(|e| format!("Index->LLVM lowering failed: {e}"))?;
    }

    // Step 4: CF -> LLVM
    {
        let pm = PassManager::new(ctx);
        pm.enable_verifier(true);
        pm.add_pass(pass::conversion::create_control_flow_to_llvm());
        pm.run(module)
            .map_err(|e| format!("CF->LLVM lowering failed: {e}"))?;
    }

    // Step 5: MemRef -> LLVM
    {
        let pm = PassManager::new(ctx);
        pm.enable_verifier(true);
        pm.add_pass(pass::conversion::create_finalize_mem_ref_to_llvm());
        pm.run(module)
            .map_err(|e| format!("MemRef->LLVM lowering failed: {e}"))?;
    }

    // Step 6: Func -> LLVM
    {
        let pm = PassManager::new(ctx);
        pm.enable_verifier(true);
        pm.add_pass(pass::conversion::create_func_to_llvm());
        pm.run(module)
            .map_err(|e| format!("Func->LLVM lowering failed: {e}"))?;
    }

    // Step 7: Reconcile casts
    {
        let pm = PassManager::new(ctx);
        pm.enable_verifier(true);
        pm.add_pass(pass::conversion::create_reconcile_unrealized_casts());
        pm.run(module)
            .map_err(|e| format!("Reconcile unrealized casts failed: {e}"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use melior::ir::operation::OperationLike;

    #[test]
    fn test_lower_add_to_llvm() {
        let ctx = crate::create_mlir_context();
        let mut module = MlirModule::parse(
            &ctx,
            r#"
            module {
                func.func @add(%arg0: i64, %arg1: i64) -> i64 {
                    %0 = arith.addi %arg0, %arg1 : i64
                    return %0 : i64
                }
            }
            "#,
        )
        .unwrap();

        lower_to_llvm_dialect(&mut module, &ctx).unwrap();
        assert!(module.as_operation().verify());

        let text = module.as_operation().to_string();
        assert!(text.contains("llvm."));
    }

    #[test]
    fn test_lower_branch_to_llvm() {
        let ctx = crate::create_mlir_context();
        let mut module = MlirModule::parse(
            &ctx,
            r#"
            module {
                func.func @branch(%arg0: i1) -> i64 {
                    cf.cond_br %arg0, ^bb1, ^bb2
                ^bb1:
                    %c1 = arith.constant 1 : i64
                    return %c1 : i64
                ^bb2:
                    %c0 = arith.constant 0 : i64
                    return %c0 : i64
                }
            }
            "#,
        )
        .unwrap();

        lower_to_llvm_dialect(&mut module, &ctx).unwrap();
        assert!(module.as_operation().verify());
        let text = module.as_operation().to_string();
        assert!(text.contains("llvm.cond_br"));
    }

    #[test]
    fn test_lower_stepwise() {
        let ctx = crate::create_mlir_context();
        let mut module = MlirModule::parse(
            &ctx,
            r#"
            module {
                func.func @mul(%arg0: i64, %arg1: i64) -> i64 {
                    %0 = arith.muli %arg0, %arg1 : i64
                    return %0 : i64
                }
            }
            "#,
        )
        .unwrap();

        lower_to_llvm_dialect_stepwise(&mut module, &ctx).unwrap();
        assert!(module.as_operation().verify());
        let text = module.as_operation().to_string();
        assert!(text.contains("llvm."));
    }

    #[test]
    fn test_lower_float_ops() {
        let ctx = crate::create_mlir_context();
        let mut module = MlirModule::parse(
            &ctx,
            r#"
            module {
                func.func @fadd(%arg0: f64, %arg1: f64) -> f64 {
                    %0 = arith.addf %arg0, %arg1 : f64
                    return %0 : f64
                }
            }
            "#,
        )
        .unwrap();

        lower_to_llvm_dialect(&mut module, &ctx).unwrap();
        assert!(module.as_operation().verify());
        let text = module.as_operation().to_string();
        assert!(text.contains("llvm.fadd"));
    }

    #[test]
    fn test_lower_comparison() {
        let ctx = crate::create_mlir_context();
        let mut module = MlirModule::parse(
            &ctx,
            r#"
            module {
                func.func @lt(%arg0: i64, %arg1: i64) -> i1 {
                    %0 = arith.cmpi slt, %arg0, %arg1 : i64
                    return %0 : i1
                }
            }
            "#,
        )
        .unwrap();

        lower_to_llvm_dialect(&mut module, &ctx).unwrap();
        assert!(module.as_operation().verify());
        let text = module.as_operation().to_string();
        assert!(text.contains("llvm.icmp"));
    }
}
