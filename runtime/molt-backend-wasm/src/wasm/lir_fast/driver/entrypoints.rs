use super::abi::LirWasmAbi;
use super::lower_lir_to_wasm_with_abi;
use crate::wasm::body::WasmBody;
use molt_tir::tir::function::TirFunction;
use molt_tir::tir::lir::LirFunction;
#[cfg(test)]
use molt_tir::tir::lower_to_lir::lower_function_to_lir;
use molt_tir::tir::lower_to_lir::lower_function_to_lir_with_inline_proof;
use molt_tir::tir::values::ValueId;
use std::collections::HashMap;

/// Lower a TIR function to WASM instructions.
///
/// Type-specialized: `I64` -> `wasm i64`, `F64` -> `wasm f64`, `DynBox` -> runtime call.
#[cfg(test)]
pub(crate) fn lower_tir_to_wasm(func: &TirFunction) -> WasmBody {
    // The generic path derives carriers from the same pure-TIR `repr_by_value`
    // authority as the boxed-i64 ABI path and LLVM. Semantic `I64` alone is not
    // a raw machine carrier; unproven ints lower as DynBox/boxed runtime values,
    // while Bool/F64 and range-proven ints keep their scalar lanes.
    let lir = lower_function_to_lir(func);
    lower_lir_to_wasm(&lir)
}

#[cfg(any(test, feature = "test-util"))]
pub(crate) fn lower_lir_to_wasm(func: &LirFunction) -> WasmBody {
    lower_lir_to_wasm_with_abi(func, LirWasmAbi::Native)
        .expect("native LIR-to-WASM lowering is total for well-formed LIR")
}

#[cfg(test)]
pub(crate) fn lower_tir_to_wasm_boxed_i64_abi(func: &TirFunction) -> Option<WasmBody> {
    let vr = crate::representation_plan::value_range_for(func);
    let repr = crate::representation_plan::repr_by_value_for(func, Some(&vr));
    lower_tir_to_wasm_boxed_i64_abi_with_proof(func, &repr, &vr)
}

/// Boxed-i64 WASM ABI lowering with the value-range proof explicitly paired to
/// the value-keyed Repr map. The production WASM fast lane uses this entry so
/// full-range raw carriers can never take the 47-bit-window checked-i64 triple.
#[cfg(feature = "wasm-backend")]
pub(crate) fn lower_tir_to_wasm_boxed_i64_abi_with_proof(
    func: &TirFunction,
    repr: &HashMap<ValueId, crate::repr::Repr>,
    inline_proof: &crate::tir::ValueRangeResult,
) -> Option<WasmBody> {
    let lir = lower_function_to_lir_with_inline_proof(func, repr, inline_proof);
    lower_lir_to_wasm_boxed_i64_abi(&lir)
}

#[cfg(feature = "wasm-backend")]
pub(crate) fn lower_lir_to_wasm_boxed_i64_abi(func: &LirFunction) -> Option<WasmBody> {
    lower_lir_to_wasm_with_abi(func, LirWasmAbi::BoxedI64)
}
