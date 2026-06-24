//! molt-tir — the backend-agnostic lower layer of the molt compiler.
//!
//! Extracted from molt-backend (decomposition program doc 21, move T1). Contains
//! the typed IR (`tir`), the SimpleIR transport (`ir`/`ir_schema`/`json_boundary`),
//! the backend-agnostic SimpleIR passes (`passes`), the representation lattice
//! (`representation_plan` + the `Repr` carrier axis), and three leaf utilities
//! (`debug_artifacts`/`process_diagnostics`/`intrinsic_symbols`). No dependency on
//! any backend; every backend crate depends on this one.

#![allow(clippy::needless_range_loop)] // index vars used in mutation / skip-set patterns
#![allow(clippy::too_many_arguments)] // refactoring signatures risks breaking callers
#![allow(clippy::type_complexity)] // complex return types in TIR CFG helpers
#![allow(clippy::should_implement_trait)] // generated op_kind enum `from_str` parsers are deliberate, not std FromStr impls

pub mod debug_artifacts;
pub mod intrinsic_symbols;
pub mod ir;
pub mod ir_schema;
pub mod json_boundary;
pub mod passes;
pub mod process_diagnostics;
pub mod representation_plan;
pub mod tir;

pub use crate::ir::{FunctionIR, OpIR, PgoProfileIR, SimpleIR, validate_simple_ir};

/// The representation lattice element (the orthogonal carrier axis to `TirType`),
/// re-exported at the crate root to mirror molt-backend's historical
/// `crate::Repr` path.
pub use crate::representation_plan::Repr;

/// The implicit FIRST parameter name the frontend prepends to every closure's
/// parameter list to carry its captured environment (the tuple of capture
/// cells). A function whose `param_names[0]` equals this marker IS a closure;
/// `param_names[1..]` are its declared Python parameters.
///
/// This is the TIR-side source of truth for the marker. It MIRRORS the frontend
/// constant `_MOLT_CLOSURE_PARAM` (`src/molt/frontend/_types.py`), which is the
/// Python-side authority that actually emits the name; the two must stay
/// byte-identical. Every backend reaches this through `molt-tir` instead of
/// re-spelling the literal.
pub const MOLT_CLOSURE_PARAM_NAME: &str = "__molt_closure__";
