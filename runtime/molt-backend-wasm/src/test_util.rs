use molt_tir::tir::lir::LirFunction;

use crate::wasm_abi::{
    RESERVED_RUNTIME_CALLABLE_COUNT, RESERVED_RUNTIME_CALLABLE_SPECS, poll_table_imports,
};

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

#[must_use]
pub fn reserved_runtime_callable_table_ref_exports(table_base: u32) -> Vec<String> {
    let poll_table_prefix = poll_table_imports()
        .filter_map(|spec| spec.poll_table_slot)
        .max()
        .unwrap_or(0)
        + 1;
    let reserved_callable_start = poll_table_prefix;
    let reserved_trampoline_start = reserved_callable_start + RESERVED_RUNTIME_CALLABLE_COUNT;
    RESERVED_RUNTIME_CALLABLE_SPECS
        .iter()
        .flat_map(|spec| {
            [
                format!(
                    "__molt_table_ref_{}",
                    table_base + reserved_callable_start + spec.index
                ),
                format!(
                    "__molt_table_ref_{}",
                    table_base + reserved_trampoline_start + spec.index
                ),
            ]
        })
        .collect()
}
