//! Escape-state analysis for TIR.
//!
//! Determines whether heap-allocated values escape the current function.
//! Nonescape does not prove immutable destruction or justify frame placement.
//! The analysis supplies alias/SROA facts without rewriting allocation storage.

mod analysis;
mod classify;

#[cfg(test)]
mod tests;

pub use analysis::analyze;
pub(crate) use analysis::{ParameterCapture, finalizer_alloc_roots, op_result_defines_del};
pub use classify::EscapeState;
