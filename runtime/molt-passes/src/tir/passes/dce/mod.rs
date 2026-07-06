//! Dead Code Elimination (DCE) pass for TIR.
//!
//! Removes operations whose results are never used by any other op or
//! terminator, provided those operations are free of side effects.
//! Iterates to a fixpoint (at most 10 rounds) to handle cascading removals.
//! Also removes blocks that are unreachable (no predecessors, excluding entry).

mod classify;
mod engine;
mod uses;

#[cfg(test)]
mod tests;

pub use engine::run;
