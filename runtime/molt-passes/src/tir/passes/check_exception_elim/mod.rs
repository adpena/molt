//! Redundant `CheckException` elimination pass.
//!
//! The frontend liberally emits `CHECK_EXCEPTION` after every statement
//! within a try block (and within functions that have a function-level
//! exception label). Many of these checks are redundant because the
//! intervening ops cannot raise: pure arithmetic, constants, variable
//! load/store, comparisons on known types, etc.
//!
//! This pass runs a small forward dataflow analysis and removes any
//! `CheckException` op that follows only non-raising ops since the
//! previous observed/cleared exception state, including across normal
//! CFG edges. Exception-handler targets stay conservatively seeded as
//! pending-possible, so handler entry semantics are preserved while
//! normal fallthrough blocks do not pay an unconditional first-poll tax.
//!
//! Targets bench_exception_heavy and other try-block-bearing loops
//! where the per-iter check_exception count drives noticeable
//! per-instruction overhead.
//!
//! ## Safety
//!
//! `CheckException` is a side-effecting op (it branches to a handler
//! when the runtime exception flag is set). Removing one is safe iff
//! no op since the previous check could have set the flag. The base
//! classifier delegates to the same op-aware TIR effects oracle that DCE
//! uses, then tightens it with local TIR facts for operations whose only
//! remaining exceptional case has been statically excluded.

pub(crate) mod classify;
mod engine;
mod flow;

#[cfg(test)]
mod tests;

pub use engine::run;
