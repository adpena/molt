//! MLIR bridge for TIR using the melior bindings.
//!
//! This crate intentionally lives outside `molt-backend` so the MLIR/LLVM stack
//! can evolve independently of the inkwell/LLVM stack used by the LLVM backend.

use melior::{
    Context as MlirContext,
    ir::{Location, Module as MlirModule, operation::OperationLike},
};

/// Create an MLIR context with the standard dialects registered.
pub fn create_mlir_context() -> MlirContext {
    let ctx = MlirContext::new();
    ctx.append_dialect_registry(&melior::dialect::DialectRegistry::new());
    ctx.load_all_available_dialects();
    ctx
}

/// Convert a TIR function to an MLIR module by parsing the textual bridge form.
pub fn tir_to_mlir<'c>(
    func: &molt_backend::tir::function::TirFunction,
    ctx: &'c MlirContext,
) -> Result<MlirModule<'c>, String> {
    let mlir_text = molt_backend::tir::mlir_compat::to_mlir_text(func);
    let _location = Location::unknown(ctx);
    MlirModule::parse(ctx, &mlir_text).ok_or_else(|| "Failed to parse TIR as MLIR".to_string())
}

/// Run MLIR verification and placeholder optimization passes on a module.
pub fn run_mlir_passes(module: &MlirModule<'_>, _ctx: &MlirContext) -> Result<(), String> {
    if module.as_operation().verify() {
        Ok(())
    } else {
        Err("MLIR verification failed".to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_context_creation() {
        let _ctx = super::create_mlir_context();
    }

    #[test]
    fn test_mlir_bridge_module_exists() {
        let _ctx = super::create_mlir_context();
    }
}
