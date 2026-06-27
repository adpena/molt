//! TIR -> SimpleIR lowering.
//!
//! This module is the canonical custody boundary for projecting optimized TIR
//! back into the SimpleIR stream consumed by the current native, WASM, and Luau
//! backend entrypoints.  It preserves SimpleIR naming facts, labels, structured
//! control flow, exception edges, and non-semantic backend metadata without
//! duplicating ownership or representation authority.

mod cfg;
mod cleanup;
mod op_lowering;
mod op_utils;
mod runner;
mod structured;

#[cfg(test)]
mod tests;

pub use self::cleanup::validate_labels;
pub use self::runner::lower_to_simple_ir;
pub use crate::tir::simple_value_names::SimpleValueNames;
