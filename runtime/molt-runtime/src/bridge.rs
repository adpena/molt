//! Direct bridge API for satellite sources compiled inside `molt-runtime`.
//!
//! Reduced stdlib profiles compile selected satellite source files directly by
//! `#[path]`. Those files depend on a `crate::bridge` access layer; this facade
//! preserves that API while routing helper families to focused submodules.

#![allow(dead_code)]

mod attrs;
mod core;
mod ext_state;
mod iteration;
mod itertools;
mod path;

pub use attrs::*;
pub use core::*;
pub use ext_state::*;
pub use iteration::*;
pub use itertools::*;
pub use path::*;
