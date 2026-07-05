//! TIR liveness analysis (RC drop-insertion substrate, design 20, Phase 2).
//!
//! Standard backward dataflow liveness over the final-SSA TIR, with one
//! domain-specific twist: **representation-filtered live sets**. A value whose
//! physical carrier holds no refcounted heap obligation — a bare `i64`
//! (`Repr::RawI64Safe`), an inline bool (`Repr::Bool`), a bare `f64`
//! (`Repr::FloatUnboxed`), the `None` singleton/sentinel, or an unreachable
//! `Repr::Never` — is excluded from the live sets. The drop pass
//! consumes these sets to place `DecRef`s; a raw scalar carries no refcount, so
//! including it would lead the drop pass to emit a `DecRef` on a register that is
//! not a NaN-boxed pointer (a type confusion). Filtering here keeps the drop
//! pass's last-use placement automatically sound for the raw lanes — the
//! overflow-peel fast loop's accumulators receive ZERO drops structurally.
//!
//! ## Dataflow
//!
//! ```text
//! LiveOut[B] = ⋃ { LiveIn[S] | S ∈ succ(B) }
//! LiveIn[B]  = (LiveOut[B] \ Kill[B]) ∪ Use[B]
//! ```
//!
//! where, restricted to the heap-carrying values:
//! * `Use[B]`  — values read by ops in `B` before any in-block definition, plus
//!   the values `B`'s terminator passes as branch/condition args, plus the
//!   values predecessors deliver to `B`'s block args (those bind `B`'s args, so
//!   they are uses *of the predecessor*, accounted via the successor's block-arg
//!   live-in — see [`live_out_of`]).
//! * `Kill[B]` — values defined by ops in `B` (op results) and `B`'s own block
//!   args (an SSA def at block entry).
//!
//! Iterated to a fixpoint over a reverse-postorder block walk (back-edges
//! converge because the transfer functions are monotone over the finite value
//! set).
//!
//! ## Block-argument (phi) semantics
//!
//! TIR uses MLIR-style block arguments instead of phi nodes: a predecessor's
//! terminator passes a list of values that bind the successor's block args on
//! entry. The passed value is a *use* in the predecessor; the block arg is a
//! *def* (kill) in the successor. [`live_out_of`] threads this precisely: a
//! value live-in to a successor's body propagates to the predecessor's live-out
//! **only if** it is not one of the successor's block args (those are killed at
//! the successor's entry and re-supplied by the edge args, which are separately
//! counted as predecessor uses).

mod api;
mod flow;
mod raw;
mod solver;

#[cfg(test)]
mod tests;

pub use api::{TirLiveness, TirLivenessResult};
pub use solver::compute_liveness;
