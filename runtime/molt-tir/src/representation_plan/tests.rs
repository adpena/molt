//! Representation-plan unit tests.
//!
//! Split move-only into cohesive sibling modules:
//! - [`scalar_facts`]: scalar/container facts derived from `FunctionIR`/`OpIR`.
//! - [`value_range`]: value-keyed `RawI64Safe` promotion via value-range
//!   analysis (the WASM/LLVM backend proof source).

mod scalar_facts;
mod value_range;
