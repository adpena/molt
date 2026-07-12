//! Scalar Evolution (SCEV) analysis for TIR — Tier-0 substrate **S6**.
//!
//! A closed-form recurrence representation for SSA values, modeled on LLVM's
//! `ScalarEvolution`. For each value the analysis computes a [`ScevExpr`]
//! describing how the value evolves; for each loop it computes a [`TripCount`].
//!
//! This is the foundation under general bounds-check elimination, induction-
//! variable strength reduction, dynamic-trip unrolling, and the
//! `MaybeBigInt → RawI64Safe` representation promotion. The immediate consumer
//! in this arc is [`super::super::passes::value_range`], which turns affine
//! recurrences and trip counts into integer ranges.
//!
//! ## What an `AddRec` is
//!
//! `AddRec { start, step, loop_header }` denotes the affine recurrence
//! `{start, +, step}` over `loop_header`: the value equals `start` on the first
//! iteration and increases by `step` each subsequent iteration. It is the SCEV
//! form of a canonical induction variable.
//!
//! ## IV detection from the post-`range_devirt` shape
//!
//! After `range_devirt` lowers `for i in range(...)` (and after `iter_devirt`
//! produces the equivalent `while i < len: ...` shape), a canonical integer
//! induction variable manifests as:
//!
//!   * a **loop-header block argument** `iv` (the SSA "phi"), whose incoming
//!     values are a loop-invariant `start` (from the preheader edge) and a
//!     `next` (from each back-edge);
//!   * `next = Add(iv, step)` computed on the back-edge block, where `step` is
//!     loop-invariant.
//!
//! When the back-edge increment carries the `no_signed_wrap` attribute (set by
//! `range_devirt` for unit steps) — or is otherwise proven not to wrap — the
//! recurrence is a sound `AddRec`. Without that proof we must NOT construct an
//! `AddRec`: a wrapping increment is not affine over the integers, and a
//! consumer that assumed monotonicity would miscompile (the loop-IV OOM
//! hazard). See [`SCEV soundness`](#soundness).
//!
//! ## <a name="soundness"></a>Soundness rules (each one prevents a miscompile)
//!
//!   1. **No-wrap requirement for `AddRec`.** A back-edge `Add(iv, step)` only
//!      forms an `AddRec` when it carries `no_signed_wrap`. Otherwise the value
//!      is `Unknown`.
//!   2. **Loop-invariant `step`.** The step must be loop-invariant (defined
//!      outside the loop or a constant). A step that itself varies per-iteration
//!      makes the recurrence non-affine.
//!   3. **Degree-2 recurrence → `Unknown`.** If the step is itself an `AddRec`
//!      (the `total += i` accumulator pattern, whose closed form is quadratic),
//!      we refuse to model it: returning `Unknown` keeps any downstream
//!      range/representation consumer conservative. Promoting such an
//!      accumulator to a bounded raw-i64 carrier is the loop-IV OOM hazard
//!      (`project_loop_iv_osc_15_baton`).
//!   4. **Single back-edge value.** The IV header-arg must receive exactly one
//!      `start` (from non-back-edge predecessors, and they must agree) and the
//!      same `next` from every back-edge. Divergent incoming values → `Unknown`.

mod builder;
mod compute;
mod index;
mod result;
mod trip_count;

#[cfg(test)]
mod tests;

pub use compute::compute_scev;
pub(crate) use compute::compute_scev_with_loop_forest;
pub use result::{ScalarEvolution, ScevResult};
pub(crate) use trip_count::find_loop_guard;
