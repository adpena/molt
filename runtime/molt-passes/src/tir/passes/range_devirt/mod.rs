//! Range loop devirtualization pass.
//!
//! Transforms `for i in range(...)` iterator protocol into direct while-loop
//! arithmetic, eliminating:
//!   - range object heap allocation
//!   - range_iterator heap allocation
//!   - per-iteration `__next__` call + StopIteration check
//!   - boxing/unboxing of the induction variable
//!
//! Pattern matched (in TIR):
//! ```text
//!   range_obj = CallBuiltin("range", args...)
//!   iter_val  = GetIter(range_obj)
//!   ...
//!   (elem, done) = IterNextUnboxed(iter_val)   // in loop header
//!   CondBranch(done, exit, body)
//! ```
//!
//! Transformed to:
//! ```text
//!   // start/stop/step materialized as ConstInt or forwarded values
//!   Branch -> header(start_val)
//!   header(i):
//!     cond = Lt(i, stop_val)    // Gt for negative step
//!     CondBranch(cond, body, exit)
//!   body:
//!     ... uses i ...
//!     next_i = Add(i, step_val)
//!     Branch -> header(next_i)
//! ```
//!
//! This runs early in the pipeline and records the scalar facts it synthesizes
//! directly in `TirFunction.value_types`; downstream passes and backends must
//! read those facts rather than legacy SimpleIR `fast_int` transport hints.

mod candidate;
mod engine;
mod transform;

#[cfg(test)]
mod tests;

pub use engine::run;
