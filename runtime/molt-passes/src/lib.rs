//! molt-passes - SimpleIR/TIR pass, fact, and orchestration layer.
//!
//! The crate owns optimization passes, analyses, fact graphs, module
//! orchestration, target/profile descriptors, and pass-cache plumbing. It sits
//! above `molt-ir` and below lowering/backend crates in the compilation DAG.

#![allow(clippy::needless_range_loop)] // index vars used in mutation / skip-set patterns
#![allow(clippy::too_many_arguments)] // refactoring signatures risks breaking callers
#![allow(clippy::type_complexity)] // complex return types in TIR CFG helpers
#![allow(clippy::should_implement_trait)] // generated op_kind enum parsers are deliberate tables

pub mod representation_facts;
pub mod tir;

pub use molt_ir::tir as ir_tir;
pub use molt_ir::{FunctionIR, OpIR, PgoProfileIR, Repr, SimpleIR, validate_simple_ir};
pub use molt_ir::{
    MOLT_CLOSURE_PARAM_NAME, debug_artifacts, intrinsic_symbols, ir, ir_schema, json_boundary,
    process_diagnostics, repr, stdlib_module_symbols,
};
