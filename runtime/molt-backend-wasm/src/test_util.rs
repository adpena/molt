use molt_tir::tir::lir::LirFunction;

pub use crate::wasm::body::{WasmBodyTestView, WasmLirFallbackReason};

#[must_use]
pub fn lower_lir_to_wasm(func: &LirFunction) -> WasmBodyTestView {
    crate::wasm::lir_fast::lower_lir_to_wasm(func).test_view()
}
