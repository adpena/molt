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
    MlirCompileOptions, MlirCompileResult, MlirOptLevel, compile_via_mlir, create_optimized_module,
    jit_execute_i64,
};

use melior::{
    Context as MlirContext,
    dialect::DialectRegistry,
    ir::{Module as MlirModule, operation::OperationLike},
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
mod tests;
