//! Constant-folding evaluation for SCCP.
//!
//! Pure functions that concretely evaluate TIR ops and admitted builtin calls
//! on already-constant operands. The SCCP lattice driver and
//! rewrite stay in the parent module; this module owns concrete evaluation.

mod builtins;
mod ops;
#[cfg(test)]
mod tests;

pub(super) use self::builtins::evaluate_builtin_call;
pub(super) use self::ops::evaluate_op;
