//! TIR liveness analysis (RC drop-insertion substrate, design 20, Phase 2).
//!
//! Standard backward dataflow liveness over the final-SSA TIR, with one
//! domain-specific twist: **representation-filtered live sets**. A value whose
//! physical carrier holds no refcounted heap obligation — a bare `i64`
//! (`Repr::RawI64Safe`), an inline bool (`Repr::Bool`), a bare `f64`
//! (`Repr::FloatUnboxed`), or the `None` singleton/sentinel — is excluded from
//! the live sets. `Repr::Never` remains the representation lattice's join
//! bottom, but is not exclusion evidence for a defined value: stale or
//! manually-authored `TirType::Never` floors to `Repr::DynBox` until an exact
//! producer proves a non-heap carrier. The drop pass
//! consumes these sets to place `DecRef`s; a raw scalar carries no refcount, so
//! including it would lead the drop pass to emit a `DecRef` on a register that is
//! not a NaN-boxed pointer (a type confusion). Filtering here keeps the drop
//! pass's last-use placement automatically sound for the raw lanes — the
//! overflow-peel fast loop's accumulators receive ZERO drops structurally.
//!
//! ## Dataflow
//!
//! ```text
//! LiveOut[B] = normal terminator-successor demand
//! LiveIn[B]  = backward transfer through B, merging exceptional demand
//!              at each exception observation before crossing earlier definitions
//! ```
//!
//! where, restricted to the heap-carrying values:
//! * `Use[B]`  — values read by ops in `B` before any in-block definition, plus
//!   the values `B`'s terminator passes as branch/condition args, plus the
//!   values predecessors deliver to `B`'s block args (those bind `B`'s args, so
//!   they are uses *of the predecessor*, accounted via the successor's block-arg
//!   live-in).
//! * `Kill[B]` — values defined by ops in `B` (op results) and `B`'s own block
//!   args (an SSA def at block entry).
//!
//! A predecessor worklist propagates each newly live root once per block,
//! using the position of its first local definition to cut off incoming
//! demand. Normal and exceptional demand remain separate at each observation
//! so cleanup can release an owner abandoned by only the exceptional path.
//!
//! ## Block-argument (phi) semantics
//!
//! TIR uses MLIR-style block arguments instead of phi nodes: a predecessor's
//! terminator passes a list of values that bind the successor's block args on
//! entry. The passed value is a *use* in the predecessor; the block arg is a
//! *def* (kill) in the successor and therefore absent from its live-in set.

mod api;
mod raw;
mod solver;

#[cfg(test)]
mod tests;

pub use api::{TirLiveness, TirLivenessResult};
pub(crate) use raw::compute_raw_scalars;
pub use solver::compute_liveness;
pub(crate) use solver::{compute_liveness_in_domain, visit_exception_liveness};
