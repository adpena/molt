pub(crate) use crate::wasm_abi_generated::LirRuntimeCall;
use crate::wasm_abi_generated::WasmNumericRuntimeSelection;
pub(super) use crate::wasm_abi_generated::{LirFixedRuntimeCall, lir_fixed_runtime_call};

pub(super) fn numeric_lir_runtime_call(selection: WasmNumericRuntimeSelection) -> LirRuntimeCall {
    selection.lir_runtime_call.unwrap_or_else(|| {
        panic!(
            "generated WASM numeric selector for {} lacks LIR runtime-call authority",
            selection.import_name
        )
    })
}
