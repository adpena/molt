//! SIMD Vectorization Hint Analysis pass for TIR.
//!
//! Scans TIR functions for loops that are safe to vectorize and annotates
//! the loop-header block's `ForIter` / `ScfFor` op with hint attributes:
//!
//! - `"vectorize" = AttrValue::Bool(true)` — loop body is vectorizable.
//! - `"reduction" = AttrValue::Str("sum"|"product"|"min"|"max"|"and"|"or")`
//!   — a simple reduction pattern was detected.
//!
//! This pass performs **hint annotation only**; actual SIMD code generation is
//! deferred to the LLVM backend which reads these attrs.
//!
//! ## Vectorizability criteria
//!
//! A loop discovered by the shared S1 `LoopForest` analysis is considered
//! vectorizable when **all** of the following hold:
//!
//! 1. Every non-structural op operates only on `I64`, `F64`, or `Bool` values.
//!    Mixed numeric types are allowed via Python-style numeric promotion (see
//!    "Mixed-type promotion" below).
//! 2. No op is a `Call`, `CallMethod`, `CallBuiltin`, or any other impure op.
//! 3. No write to non-local memory (`StoreAttr`, `StoreIndex`, `DelAttr`,
//!    `DelIndex`, `Free`, `IncRef`, `DecRef`).
//! 4. No generator ops (`Yield`, `YieldFrom`), exception ops (`Raise`,
//!    `CheckException`), or import ops.
//!
//! ## Mixed-type promotion
//!
//! Python's numeric tower promotes `bool → int → float`. SIMD ISAs (SSE2/AVX2/
//! AVX-512/NEON/SVE) all support both i64 and f64 lanes at the same vector
//! bit-width (e.g. AVX2 supplies both 4xi64 and 4xf64 in a 256-bit register),
//! and provide cheap `sitofp` lane-wise conversions. We classify each loop's
//! body by the join of every numeric value it touches:
//!
//! - All `I64` (and `Bool`, which zext-promotes to i64) → vectorize as i64 lanes.
//! - All `F64`                                          → vectorize as f64 lanes.
//! - Mixed `{I64, Bool} ∪ {F64}`                        → vectorize as f64 lanes
//!   with a `promoted = true` hint so the backend inserts `sitofp` on the
//!   integer-typed lane loads. The total vector bit-width stays the same; the
//!   lane count is unchanged because i64 and f64 share the same lane width.
//!
//! This lift is correctness-preserving: float arithmetic on integers that fit
//! in 53 mantissa bits is exact, matching CPython's behaviour for the values
//! that participate in such mixed loops in practice. The LIR backend is free
//! to widen / narrow the chosen lane count based on target features; we emit
//! the conservative 2-lane (128-bit) minimum.
//!
//! ## Reduction detection
//!
//! A sum reduction is detected when there exists a block argument `acc` that
//! is the sole result of an `Add` op whose operands include `acc` itself (the
//! classic accumulator += value pattern in SSA form via a block back-arg).

mod analysis;
mod engine;

#[cfg(test)]
mod tests;

pub use analysis::{ReductionOp, VectorizationInfo};
pub use engine::run;
