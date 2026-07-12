//! SROA - Scalar Replacement of Aggregates.
//!
//! SROA promotes the fields of a proven-non-escaping object out of heap/stack
//! memory and into pure SSA register values, then deletes the now-redundant
//! stores. It is the pass that closes the `bench_struct` allocation cliff after
//! MemGVN has forwarded away typed-slot loads.
//!
//! The pass fails closed on three obligations:
//! - the object is unobserved after alias-root canonicalization,
//! - every removed store is refcount-neutral,
//! - every removed store is a recognized typed-slot store.
//!
//! DCE removes the now-unreferenced `ObjectNewBoundStack` after SROA removes
//! the stores.

mod classify;
mod engine;
mod report;

#[cfg(test)]
mod tests;

pub use engine::run;
