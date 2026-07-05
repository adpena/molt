//! Dead block-argument pruning.
//!
//! TIR uses MLIR-style block arguments as its phi representation. Earlier
//! passes can shrink executable use sites while leaving join/handler/resume
//! block signatures wide. Those dead payload lanes are semantically inert, but
//! they are still lowered to SimpleIR `store_var`/`load_var` traffic on every
//! predecessor edge, which can dominate native codegen memory for large
//! ecosystem functions. This pass removes those lanes at the TIR authority
//! layer and rewrites every edge payload surface that can bind block args:
//! explicit terminators, state-dispatch edges, and implicit exception-transfer
//! op operands.

mod edge;
mod engine;
mod liveness;

#[cfg(test)]
mod tests;

pub use engine::run;
