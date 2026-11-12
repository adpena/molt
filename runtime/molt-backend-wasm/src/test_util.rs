use crate::wasm_abi::RESERVED_RUNTIME_CALLABLE_SPECS;

#[must_use]
pub fn reserved_runtime_callable_import_names() -> Vec<&'static str> {
    RESERVED_RUNTIME_CALLABLE_SPECS
        .iter()
        .map(|spec| spec.import.name())
        .collect()
}
