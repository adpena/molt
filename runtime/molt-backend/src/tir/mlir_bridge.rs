//! MLIR bridge — converts TIR to actual MLIR operations via the melior crate.
//! This enables using MLIR's optimization passes (loop tiling, fusion, etc.)
//! on TIR programs.

#[cfg(feature = "mlir")]
use melior::{
    ir::{Location, Module as MlirModule},
    Context as MlirContext,
};

/// Create an MLIR context with the standard dialects registered.
#[cfg(feature = "mlir")]
pub fn create_mlir_context() -> MlirContext {
    let ctx = MlirContext::new();
    // Register standard dialects
    ctx.append_dialect_registry(&melior::dialect::DialectRegistry::new(&ctx));
    ctx.load_all_available_dialects();
    ctx
}

/// Convert a TIR function to an MLIR module.
/// This is the bridge that enables MLIR optimization passes on TIR.
#[cfg(feature = "mlir")]
pub fn tir_to_mlir(
    func: &super::function::TirFunction,
    ctx: &MlirContext,
) -> Result<MlirModule, String> {
    // Parse the MLIR text representation we already generate
    let mlir_text = super::mlir_compat::to_mlir_text(func);
    let _location = Location::unknown(ctx);
    MlirModule::parse(ctx, &mlir_text)
        .ok_or_else(|| "Failed to parse TIR as MLIR".to_string())
}

/// Run MLIR optimization passes on a module.
#[cfg(feature = "mlir")]
pub fn run_mlir_passes(module: &MlirModule, _ctx: &MlirContext) -> Result<(), String> {
    // The actual passes require registering the molt dialect with MLIR,
    // which needs a custom dialect definition. For Phase 1 of MLIR integration,
    // we validate that the TIR serialization round-trips through MLIR.
    if module.as_operation().verify() {
        Ok(())
    } else {
        Err("MLIR verification failed".to_string())
    }
}

// Stubs for non-MLIR builds so downstream code can unconditionally reference this module.

/// Placeholder context type when the `mlir` feature is disabled.
#[cfg(not(feature = "mlir"))]
pub struct MlirContextStub;

/// Create a no-op MLIR context stub (mlir feature disabled).
#[cfg(not(feature = "mlir"))]
pub fn create_mlir_context() -> MlirContextStub {
    MlirContextStub
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_context_creation_stub() {
        // Without the mlir feature, we get the stub
        let _ctx = super::create_mlir_context();
        // Should compile and return without error
    }

    #[test]
    fn test_mlir_bridge_module_exists() {
        // Verify the module is reachable and the stub types are usable
        let ctx = super::create_mlir_context();
        #[cfg(not(feature = "mlir"))]
        {
            let _: &super::MlirContextStub = &ctx;
        }
    }
}
