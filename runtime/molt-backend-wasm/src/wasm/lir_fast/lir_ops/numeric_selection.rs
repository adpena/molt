use crate::wasm_abi_generated::{WasmNumericRuntimeSelection, wasm_numeric_runtime_selection};
use molt_tir::tir::ops::OpCode;

pub(super) fn numeric_selection_for_opcode(opcode: OpCode) -> WasmNumericRuntimeSelection {
    let kind = crate::tir::op_kinds_generated::opcode_canonical_kind_table(opcode);
    wasm_numeric_runtime_selection(kind)
        .unwrap_or_else(|| panic!("missing generated WASM numeric selector for {kind}"))
}
