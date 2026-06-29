//! molt-ir - leaf IR vocabulary and transport for the molt compiler.
//!
//! This crate is the zero-workspace-dependency data-model floor of the compiler
//! crate graph. It owns TIR vocabulary, SimpleIR transport, generated op-kind
//! facts, representation vocabulary, and std-only diagnostics shared by passes,
//! lowering, backends, and tooling.

#![allow(clippy::needless_range_loop)] // index vars used in mutation / skip-set patterns
#![allow(clippy::too_many_arguments)] // refactoring signatures risks breaking callers
#![allow(clippy::type_complexity)] // complex return types in TIR CFG helpers
#![allow(clippy::should_implement_trait)] // generated op_kind enum parsers are deliberate tables

pub mod debug_artifacts;
pub mod intrinsic_symbols;
pub mod ir;
pub mod ir_schema;
pub mod json_boundary;
pub mod native_callable_abi;
pub mod process_diagnostics;
pub mod repr;
pub mod stdlib_module_symbols;
pub mod tir;

pub use crate::ir::{FunctionIR, OpIR, PgoProfileIR, SimpleIR, validate_simple_ir};
pub use crate::repr::Repr;

/// The implicit FIRST parameter name the frontend prepends to every closure's
/// parameter list to carry its captured environment.
pub const MOLT_CLOSURE_PARAM_NAME: &str = "__molt_closure__";
