//! TIR to WASM type-specialized lowering.
//!
//! Converts a [`TirFunction`] into WASM instructions using the `wasm-encoder` crate.
//! The key insight: TIR carries refined type information from optimization passes,
//! so we can emit **native WASM arithmetic** for unboxed scalars instead of falling
//! back to runtime dispatch calls for every operation.
//!
//! ## Type mapping
//!
//! | TirType     | WASM ValType | Notes                          |
//! |-------------|-------------|--------------------------------|
//! | I64         | i64         | Native 64-bit integer          |
//! | F64         | f64         | Native 64-bit float            |
//! | Bool        | i32         | 0 or 1                         |
//! | None        | i64         | Sentinel constant              |
//! | DynBox      | i64         | NaN-boxed runtime value        |
//! | Ref64       | i64         | Runtime reference word         |
//! | Str/List/... | i64         | Heap pointer as i64            |
//!
//! ## SSA to stack machine
//!
//! TIR is register-based SSA; WASM is a stack machine. We allocate one WASM local
//! per SSA value and emit explicit local.get/local.set around each operation.
//! A peephole pass (`peephole_set_get_to_tee`) runs after emission to collapse
//! `local.set X; local.get X` pairs into `local.tee X`, eliminating redundant
//! stack traffic.

mod driver;
mod lir_context;
mod lir_control;
mod lir_ops;
mod lir_scalar;
mod peephole;

#[cfg(any(test, feature = "test-util"))]
pub(crate) use driver::lower_lir_to_wasm;
pub(crate) use driver::lower_tir_to_wasm_boxed_i64_abi_with_proof;
#[cfg(test)]
pub(crate) use driver::{lower_tir_to_wasm, lower_tir_to_wasm_boxed_i64_abi};

#[cfg(test)]
mod tests;
