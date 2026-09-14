//! SROA - Scalar Replacement of Aggregates.
//!
//! SROA promotes the fields of a proven-non-escaping boxed object out of heap
//! memory and into pure SSA register values, then deletes the complete object,
//! its aliases, stores, and RC after MemGVN has forwarded away typed-slot loads.
//!
//! The pass fails closed on four obligations:
//! - allocation is callback-free according to the shared effects authority,
//! - the object is unobserved after alias-root canonicalization,
//! - every removed store remains refcount-neutral after boxing into its field,
//! - every removed store is a recognized typed-slot store within its allocation's
//!   fixed field extent; class layouts reserve a trailing dictionary word.
//! Allocation and field-extent admission are shared with DSE and backend field
//! store planning; stable block compaction removes admitted operations in linear time.
//! Finalizer-bearing roots are excluded even if an upstream artifact marks
//! their allocation as stack storage.
//!
//! Class construction currently seals metadata and owns a class reference, so
//! its effects block erasure even when every instance field is neutral.
//! A surviving observation preserves the entire candidate's ownership. SROA
//! never relies on later DCE to repair an intermediate heap allocation leak.

mod engine;
mod report;

#[cfg(test)]
mod tests;

pub use engine::run;
