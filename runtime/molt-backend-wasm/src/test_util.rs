use molt_tir::tir::lir::LirFunction;

use crate::wasm_abi::RESERVED_RUNTIME_CALLABLE_SPECS;

pub use crate::wasm::body::{WasmBodyTestView, WasmLirFallbackReason};

#[must_use]
pub fn lower_lir_to_wasm(func: &LirFunction) -> WasmBodyTestView {
    crate::wasm::lir_fast::lower_lir_to_wasm(func).test_view()
}

#[must_use]
pub fn reserved_runtime_callable_import_names() -> Vec<&'static str> {
    RESERVED_RUNTIME_CALLABLE_SPECS
        .iter()
        .map(|spec| spec.import_name)
        .collect()
}
