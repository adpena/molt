//! Perceus-style reuse analysis pass for TIR.
//!
//! Based on "Perceus: Garbage Free Reference Counting with Reuse" (Reinking
//! et al., MSR, PLDI 2021).
//!
//! When a `DecRef(x)` would free an object and the immediately following
//! allocation produces an object of compatible size, we can reuse the memory
//! instead of freeing and reallocating. This pass performs the analysis only:
//! it identifies reuse candidates and annotates `DecRef`/`Alloc` pairs with
//! metadata for downstream lowering.

mod annotate;
mod compat;
mod engine;
mod scan;

#[cfg(test)]
mod tests;

pub use annotate::annotate;
pub use engine::run;
pub use scan::{ReuseCandidate, analyze};
