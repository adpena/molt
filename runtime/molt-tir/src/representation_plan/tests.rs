//! Representation-plan unit tests.
//!
//! Split move-only into cohesive sibling modules:
//! - [`scalar_facts`]: scalar/container facts derived from `FunctionIR`/`OpIR`.
//! - [`value_range`]: value-keyed `RawI64Safe` promotion via value-range
//!   analysis (the LLVM backend proof source).
//! - [`representation_unification`]: the int-lane carrier unification gate.

mod representation_unification;
mod scalar_facts;
mod value_range;
