//! Global Value Numbering (GVN) for TIR.
//!
//! Assigns a canonical "value number" to each computation.  If two operations
//! compute the same result (same opcode, same operand value numbers), the
//! second is replaced with a Copy of the first.  This subsumes common
//! subexpression elimination (CSE) and catches redundancies that SCCP misses.
//!
//! Algorithm: dominator-tree-scoped hash-based value numbering.  A scoped
//! hash table is maintained as the dominator tree is walked in pre-order.
//! Each block inherits the leader table of its immediate dominator (entries
//! defined in dominating blocks remain visible) and contributes its own new
//! entries.  On exit from a block, the entries it contributed are removed,
//! restoring the parent scope — so values are only propagated to blocks the
//! defining block actually dominates.  This catches cross-block redundancy
//! (same `a + b` in entry and a dominated body block) without ever exposing
//! a value to a non-dominated sibling.
//!
//! Only pure (side-effect-free) operations are candidates for numbering.
//! Side-effecting ops (calls, stores, imports) are always preserved.
//!
//! Reference: Briggs, Cooper, Simpson — "Value Numbering" (1997).
//! LLVM's GVN uses an analogous scoped-hash-table walk over the dominator
//! tree (see `llvm/lib/Transforms/Scalar/GVN.cpp::ValueTable`).

mod engine;
mod keys;

#[cfg(test)]
mod tests;

pub use engine::run;
