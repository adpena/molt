//! TIR to WASM type-specialized lowering.
//!
//! Converts a [`TirFunction`] into WASM instructions using the `wasm-encoder` crate.
//! The backend-neutral, value-keyed representation plan supplies physical carriers
//! from exact producer facts and integer range/overflow proofs. These carriers
//! authorize **native WASM arithmetic**; semantic type annotations alone do not.
//!
//! ## Carrier mapping
//!
//! | Repr              | WASM ValType | Notes                              |
//! |-------------------|--------------|------------------------------------|
//! | RawI64Safe        | i64          | Proven inline-int47 integer        |
//! | RawI64FullDeopt    | i64          | Checked full-i64 integer carrier   |
//! | FloatUnboxed      | f64          | Exact float producer               |
//! | Bool              | i32          | Exact Boolean producer, 0 or 1     |
//! | MaybeBigInt       | i64          | Boxed integer, including BigInt    |
//! | DynBox            | i64          | NaN-boxed runtime value            |
//!
//! Annotation-only float/Boolean parameters stay boxed, even beside exact
//! scalar producers. The boxed-i64 function ABI materializes raw carriers at
//! its boundaries without changing their internal representation authority.
//!
//! ## SSA to stack machine
//!
//! TIR is register-based SSA; WASM is a stack machine. We allocate one WASM local
//! per SSA value and emit explicit local.get/local.set around each operation.
//! A peephole pass (`peephole_set_get_to_tee`) runs after emission to collapse
//! `local.set X; local.get X` pairs into `local.tee X`, eliminating redundant
//! stack traffic.

mod driver;
mod function_emit;
mod lir_context;
mod lir_control;
mod lir_ops;
mod lir_runtime_ops;
mod lir_scalar;
mod peephole;
mod plan;
mod runtime_calls;

#[cfg(any(test, feature = "test-util"))]
pub(crate) use driver::lower_lir_to_wasm;
#[cfg(test)]
pub(crate) use driver::{lower_tir_to_wasm, lower_tir_to_wasm_boxed_i64_abi};
pub(in crate::wasm) use function_emit::try_emit_planned_lir_fast_body;
pub(crate) use plan::{
    WasmFunctionLoweringPlan, WasmFunctionLoweringPlans,
    compute_lir_wasm_lowering_plans_from_final_ir_with_escaped,
    is_production_lir_wasm_fast_path_name,
};
pub(crate) use runtime_calls::LirRuntimeCall;

#[cfg(test)]
mod tests;
