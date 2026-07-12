use super::LirLowerCtx;
use crate::wasm::body::WasmLirFallbackReason;
use crate::wasm::const_materialization::WasmConstMaterialization;
use crate::wasm::lir_fast::LirRuntimeCall;

impl LirLowerCtx<'_> {
    /// Emit a typed runtime-import call. This is how the LIR fast lane reaches
    /// runtime helpers (e.g. `int_from_i64` for the overflow-safe box) without
    /// bailing the whole function to the generic path.
    pub(in crate::wasm::lir_fast) fn emit_runtime_call(&mut self, call: LirRuntimeCall) {
        self.instructions.push_runtime_import_call(call);
    }

    pub(in crate::wasm::lir_fast) fn emit_bail_to_generic_path(
        &mut self,
        reason: WasmLirFallbackReason,
    ) {
        self.instructions.push_bail_to_generic_path(reason);
    }

    pub(in crate::wasm::lir_fast) fn emit_const_materialization(
        &mut self,
        materialization: WasmConstMaterialization,
    ) {
        self.instructions
            .push_const_materialization(materialization);
    }
}
