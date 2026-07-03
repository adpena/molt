//! Escape analysis pass for TIR.
//!
//! Determines whether heap-allocated values escape the current function.
//! Values that don't escape (`NoEscape`) are rewritten from `Alloc` to
//! `StackAlloc`, and their `IncRef`/`DecRef` ops are elided.

mod analysis;
mod apply;
mod classify;

#[cfg(test)]
mod tests;

pub use analysis::analyze;
pub(crate) use analysis::finalizer_alloc_roots;
pub use apply::{apply, run};
pub use classify::EscapeState;
