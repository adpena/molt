//! Constant-folding evaluation for SCCP.
//!
//! Pure functions that concretely evaluate TIR ops, pure builtin calls, and
//! pure method calls on already-constant operands. The SCCP lattice driver and
//! rewrite stay in the parent module; this module owns concrete evaluation.

mod builtins;
mod methods;
mod ops;
#[cfg(test)]
mod tests;

pub(super) use self::builtins::evaluate_builtin_call;
pub(super) use self::methods::evaluate_method_call;
pub(super) use self::ops::evaluate_op;
