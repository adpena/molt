//! Rust source backend leaf crate.
//!
//! This crate owns Rust source emission and validation. The driver crate
//! re-exports `rust` when the `rust-backend` feature is enabled.

#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]

pub use molt_ir::{FunctionIR, OpIR, SimpleIR, ir};
pub use molt_tir::representation_plan;

pub mod rust;
