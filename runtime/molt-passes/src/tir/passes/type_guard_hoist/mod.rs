//! Type Guard Hoisting pass for TIR.
//!
//! Hoists TypeGuard ops out of loops when the guarded value is loop-invariant.
//!
//! Loop headers and natural-loop bodies come from the shared
//! [`LoopForest`](crate::tir::analysis::LoopForest) analysis. The pass derives
//! a loop preheader from the header predecessors outside that canonical body,
//! and uses immediate dominators to prove a guarded value is available before
//! the loop.

mod defs;
mod engine;
mod loops;

#[cfg(test)]
mod tests;

pub use engine::run;
