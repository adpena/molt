//! Deforestation / Iterator Fusion Pass.
//!
//! Eliminates intermediate data structures in functional-style Python code by
//! fusing generator/iterator chains into single loops.
//!
//! ```python
//! # Before fusion:
//! result = sum(x*x for x in data if x > 0)
//! # Creates: generator for map, generator for filter, iteration for sum
//!
//! # After fusion: single loop
//! acc = 0
//! for x in data:
//!     if x > 0:
//!         acc += x * x
//! result = acc
//! ```
//!
//! Patterns detected:
//! 1. `sum(genexpr)` → accumulator loop
//! 2. `list(genexpr)` → preallocated list + append loop
//! 3. `any(genexpr)` / `all(genexpr)` → early-exit loop
//! 4. `min(genexpr)` / `max(genexpr)` → tracking loop
//!
//! Fusion-barrier requirement: only fuses when the loop body has no cross-
//! iteration/control-state barriers. The barrier classifier is generated from
//! `op_kinds.toml` and is deliberately distinct from the side-effecting and
//! may-throw classifiers because fusion preserves per-element evaluation order.

mod fusion;
mod tuple_scalarize;

#[cfg(test)]
mod tests;

pub use fusion::run;
pub use tuple_scalarize::run_tuple_scalarize;
