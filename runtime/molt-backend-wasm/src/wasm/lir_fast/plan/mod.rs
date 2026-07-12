use crate::SimpleIR;
use crate::wasm::body::{WasmBody, WasmLirFallbackReason};
use std::collections::{BTreeMap, BTreeSet};

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

pub(crate) type WasmFunctionLoweringPlans = BTreeMap<String, WasmFunctionLoweringPlan>;

pub(crate) fn prepare_lir_wasm_fast_plan(
    tir_func: &crate::tir::function::TirFunction,
) -> WasmFunctionLoweringPlan {
    let mut refined = tir_func.clone();
    crate::tir::type_refine::refine_types(&mut refined);
    // Drive the LIR carrier derivation from the PROVEN `repr_by_value` (the
    // single source of truth shared with LLVM), so `LirRepr::I64` is assigned
    // only to proven raw-i64 carriers (`RawI64Safe` or `RawI64FullDeopt`).
    // Arithmetic still consults the inline-window proof before taking unchecked
    // machine ops. The proof comes from the value-range analysis computed on
    // this exact `tir_func` (the same source the LLVM `LlvmReprFacts::build`
    // uses), so WASM and LLVM agree per `ValueId`. An unproven `int`
    // (`MaybeBigInt`) lowers to `DynBox`; its arithmetic emits a typed
    // generic-path bail, which is rejected below so the function falls back to
    // the IntFastLane-guarded slow path (correctness preserved; the unsound
    // bare op is un-emittable here).
    //
    // Refinement is owned here, immediately before proof derivation, so the
    // value-range/repr facts and LIR lowering cannot observe different result
    // types for checked multi-result ops.
    let vr = crate::representation_plan::value_range_for(&refined);
    let repr = crate::representation_plan::repr_by_value_for(&refined, Some(&vr));
    let Some(output) =
        super::driver::lower_tir_to_wasm_boxed_i64_abi_with_proof(&refined, &repr, &vr)
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

pub(crate) fn compute_lir_wasm_lowering_plans_from_final_ir_with_escaped(
    ir: &SimpleIR,
    escaped_callable_targets: &BTreeSet<String>,
) -> WasmFunctionLoweringPlans {
    let mut plans = BTreeMap::new();
    for func_ir in &ir.functions {
        if func_ir.is_extern || !is_production_lir_wasm_fast_path_name(&func_ir.name) {
            continue;
        }
        if escaped_callable_targets.contains(&func_ir.name) {
            plans.insert(
                func_ir.name.clone(),
                WasmFunctionLoweringPlan::Generic {
                    reason: WasmLirFallbackReason::EscapedCallableTarget,
                },
            );
            continue;
        }
        let tir_func = crate::tir::lower_from_simple::lower_to_tir(func_ir);
        plans.insert(func_ir.name.clone(), prepare_lir_wasm_fast_plan(&tir_func));
    }
    plans
}

pub(crate) fn is_production_lir_wasm_fast_path_name(func_name: &str) -> bool {
    func_name.contains("____molt_globals_builtin__")
}

#[cfg(all(test, feature = "wasm-backend"))]
mod tests;
