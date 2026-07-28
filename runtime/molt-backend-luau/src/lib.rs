//! Luau source backend leaf crate.
//!
//! This crate owns Luau source emission, source validation, target-specific
//! SimpleIR rewrites, and Luau support-matrix source text. The driver crate
//! re-exports `luau` when the `luau-backend` feature is enabled.

#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]

pub use molt_ir::{ExecutionContextPolicy, FunctionIR, OpIR, SimpleIR, ir, repr};
pub use molt_tir::{representation_plan, tir};

pub mod luau;
