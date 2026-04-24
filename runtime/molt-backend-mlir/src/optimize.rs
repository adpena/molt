//! MLIR optimization pass pipeline.
//!
//! Applies standard MLIR optimization passes to a module before lowering to
//! LLVM dialect. These passes operate at the func/arith/cf/scf dialect level,
//! providing target-independent optimizations that LLVM cannot perform because
//! it lacks the higher-level structural information.
//!
//! Pass ordering follows MLIR best practices:
//! 1. Canonicalization (algebraic simplification, dead branch elimination)
//! 2. CSE (common subexpression elimination)
//! 3. SCCP (sparse conditional constant propagation)
//! 4. LICM (loop-invariant code motion)
//! 5. Mem2Reg (promote memory to SSA registers)
//! 6. Symbol DCE (eliminate dead private functions)
//! 7. Inlining (inline small functions)
//! 8. Second round of canonicalize + CSE to clean up after inlining
//! 9. Dead value removal

use melior::{
    Context as MlirContext,
    ir::Module as MlirModule,
    pass::{self, PassManager},
};

/// Optimization level controlling the aggressiveness of MLIR passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OptLevel {
    /// No optimization. Only verification.
    O0,
    /// Basic optimizations: canonicalize + CSE.
    O1,
    /// Full optimization pipeline.
    O2,
    /// Aggressive: O2 + extra inlining rounds.
    O3,
}

/// Run the standard MLIR optimization pipeline on a module.
///
/// The module must already be in standard dialects (func/arith/cf/scf).
/// After this, the module is ready for lowering to LLVM dialect.
pub fn run_optimization_passes(
    module: &mut MlirModule<'_>,
    ctx: &MlirContext,
) -> Result<(), String> {
    run_optimization_passes_at_level(module, ctx, OptLevel::O2)
}

/// Run optimization passes at a specific optimization level.
pub fn run_optimization_passes_at_level(
    module: &mut MlirModule<'_>,
    ctx: &MlirContext,
    level: OptLevel,
) -> Result<(), String> {
    if level == OptLevel::O0 {
        return Ok(());
    }

    let pm = PassManager::new(ctx);
    pm.enable_verifier(true);

    // ---- Round 1: Core optimizations ----

    // Canonicalization: algebraic simplification, constant folding, dead branch
    // elimination, operation normalization.
    pm.add_pass(pass::transform::create_canonicalizer());

    // Common subexpression elimination.
    pm.add_pass(pass::transform::create_cse());

    if level >= OptLevel::O2 {
        // Sparse conditional constant propagation: propagates constants through
        // control flow, eliminates dead branches with known conditions.
        pm.add_pass(pass::transform::create_sccp());

        // Loop-invariant code motion: hoists operations out of loops when safe.
        pm.add_pass(pass::transform::create_loop_invariant_code_motion());

        // Promote stack allocations to SSA registers.
        pm.add_pass(pass::transform::create_mem_2_reg());

        // Eliminate dead private symbols (functions that are never called).
        pm.add_pass(pass::transform::create_symbol_dce());

        // Dead value removal.
        pm.add_pass(pass::transform::create_remove_dead_values());
    }

    if level >= OptLevel::O2 {
        // Inlining pass: inlines small functions into callers.
        pm.add_pass(pass::transform::create_inliner());

        // ---- Round 2: Post-inline cleanup ----
        // After inlining, re-run canonicalize + CSE to clean up redundancies
        // introduced by inlining.
        pm.add_pass(pass::transform::create_canonicalizer());
        pm.add_pass(pass::transform::create_cse());
    }

    if level >= OptLevel::O3 {
        // Aggressive: additional LICM and SCCP after inlining.
        pm.add_pass(pass::transform::create_sccp());
        pm.add_pass(pass::transform::create_loop_invariant_code_motion());

        // SCF-specific optimizations.
        pm.add_pass(pass::scf::create_scf_for_loop_canonicalization());
        pm.add_pass(pass::scf::create_scf_for_loop_range_folding());

        // Final cleanup round.
        pm.add_pass(pass::transform::create_canonicalizer());
        pm.add_pass(pass::transform::create_cse());
        pm.add_pass(pass::transform::create_remove_dead_values());
    }

    pm.run(module)
        .map_err(|e| format!("MLIR optimization pass pipeline failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use melior::ir::{Location, operation::OperationLike};

    #[test]
    fn test_o0_is_noop() {
        let ctx = crate::create_mlir_context();
        let location = Location::unknown(&ctx);
        let mut module = MlirModule::new(location);
        run_optimization_passes_at_level(&mut module, &ctx, OptLevel::O0).unwrap();
        assert!(module.as_operation().verify());
    }

    #[test]
    fn test_o1_on_empty_module() {
        let ctx = crate::create_mlir_context();
        let location = Location::unknown(&ctx);
        let mut module = MlirModule::new(location);
        run_optimization_passes_at_level(&mut module, &ctx, OptLevel::O1).unwrap();
        assert!(module.as_operation().verify());
    }

    #[test]
    fn test_o2_on_empty_module() {
        let ctx = crate::create_mlir_context();
        let location = Location::unknown(&ctx);
        let mut module = MlirModule::new(location);
        run_optimization_passes_at_level(&mut module, &ctx, OptLevel::O2).unwrap();
        assert!(module.as_operation().verify());
    }

    #[test]
    fn test_o3_on_empty_module() {
        let ctx = crate::create_mlir_context();
        let location = Location::unknown(&ctx);
        let mut module = MlirModule::new(location);
        run_optimization_passes_at_level(&mut module, &ctx, OptLevel::O3).unwrap();
        assert!(module.as_operation().verify());
    }

    #[test]
    fn test_optimize_simple_function() {
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

        run_optimization_passes(&mut module, &ctx).unwrap();
        assert!(module.as_operation().verify());
    }

    #[test]
    fn test_canonicalize_constant_fold() {
        let ctx = crate::create_mlir_context();
        let mut module = MlirModule::parse(
            &ctx,
            r#"
            module {
                func.func @folded() -> i64 {
                    %c10 = arith.constant 10 : i64
                    %c32 = arith.constant 32 : i64
                    %sum = arith.addi %c10, %c32 : i64
                    return %sum : i64
                }
            }
            "#,
        )
        .unwrap();

        run_optimization_passes(&mut module, &ctx).unwrap();
        let text = module.as_operation().to_string();
        // After constant folding, the add should be replaced with constant 42.
        assert!(text.contains("42"));
    }
}
