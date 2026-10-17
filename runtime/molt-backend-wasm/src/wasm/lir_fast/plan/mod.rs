use crate::wasm::body::{WasmBody, WasmLirFallbackReason};

#[derive(Debug, Clone)]
pub(crate) enum WasmFunctionLoweringPlan {
    LirFast(WasmBody),
    Generic { reason: WasmLirFallbackReason },
}

impl WasmFunctionLoweringPlan {
    #[cfg(test)]
    pub(crate) fn lir_fast_body(&self) -> Option<&WasmBody> {
        match self {
            Self::LirFast(body) => Some(body),
            Self::Generic { .. } => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn generic_reason(&self) -> Option<WasmLirFallbackReason> {
        match self {
            Self::LirFast(_) => None,
            Self::Generic { reason } => Some(*reason),
        }
    }
}

/// Consume the already refined final body. Analysis is value-keyed and never
/// changes or re-optimizes the body whose generic projection will be emitted.
pub(crate) fn prepare_lir_wasm_fast_plan(
    tir_func: &crate::tir::function::TirFunction,
    repr: &std::collections::HashMap<crate::tir::values::ValueId, crate::repr::Repr>,
    ranges: &crate::tir::ValueRangeResult,
) -> WasmFunctionLoweringPlan {
    let Some(output) =
        super::driver::lower_tir_to_wasm_boxed_i64_abi_with_proof(tir_func, repr, ranges)
    else {
        return WasmFunctionLoweringPlan::Generic {
            reason: WasmLirFallbackReason::BoxedI64AbiUnsupported,
        };
    };
    if let Some(reason) = output.bail_to_generic_reason() {
        WasmFunctionLoweringPlan::Generic { reason }
    } else {
        WasmFunctionLoweringPlan::LirFast(output)
    }
}

pub(crate) fn is_production_lir_wasm_fast_path_name(func_name: &str) -> bool {
    func_name.contains("____molt_globals_builtin__")
}

#[cfg(all(test, feature = "wasm-backend"))]
mod tests;
