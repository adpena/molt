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
//!
//! Admission. `range_new`'s operands are `range()`'s converted bounds, exact
//! ints by construction (the frontend converts every bound it cannot prove an
//! exact int through `operator_index`), so its own conversions are identities
//! and the induction variable is a semantic `int`. The pass removes the range
//! object and its iterator, so neither may have another use, and it keeps
//! `range_new` unless the step is a nonzero constant: the step's sign picks the
//! comparison, and a zero step must raise `range()`'s ValueError.

mod candidate;
mod engine;
mod transform;

#[cfg(test)]
mod tests;

pub use engine::run;
