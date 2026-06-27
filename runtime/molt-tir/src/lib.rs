//! molt-tir — the backend-agnostic lower layer of the molt compiler.
//!
//! Extracted from molt-backend (decomposition program doc 21, moves T1/S1).
//! This residual crate owns lowering and representation-plan logic; pass/fact
//! orchestration lives in `molt-passes`, while leaf IR vocabulary and SimpleIR
//! transport live in `molt-ir`.

#![allow(clippy::needless_range_loop)] // index vars used in mutation / skip-set patterns
#![allow(clippy::too_many_arguments)] // refactoring signatures risks breaking callers
#![allow(clippy::type_complexity)] // complex return types in TIR CFG helpers
#![allow(clippy::should_implement_trait)] // generated op_kind enum `from_str` parsers are deliberate, not std FromStr impls

pub mod passes;
pub mod representation_plan;
pub mod tir;

pub use molt_ir::{FunctionIR, OpIR, PgoProfileIR, SimpleIR, validate_simple_ir};
pub use molt_ir::{
    MOLT_CLOSURE_PARAM_NAME, debug_artifacts, intrinsic_symbols, ir, ir_schema, json_boundary,
    process_diagnostics, repr, stdlib_module_symbols,
};

/// The representation lattice element (the orthogonal carrier axis to `TirType`),
/// re-exported at the crate root to mirror molt-backend's historical
/// `crate::Repr` path.
pub use molt_ir::repr::Repr;
