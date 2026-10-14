//! Direct bridge API for satellite sources compiled inside `molt-runtime`.
//!
//! Reduced stdlib profiles compile selected satellite source files directly by
//! `#[path]`. Those files depend on a `crate::bridge` access layer; this facade
//! preserves that API while routing helper families to focused submodules.

// A test build with every satellite extracted compiles this facade only for
// the differential tests in `path_bridge.rs`, which call a few items. HF-87
// owns gating each item by the satellites that call it.
#![cfg_attr(
    all(
        test,
        feature = "stdlib_http",
        feature = "stdlib_itertools",
        feature = "stdlib_path"
    ),
    allow(dead_code)
)]

mod attrs;
mod core;
mod ext_state;
mod iteration;
mod itertools;
mod path;

// Satellite source selection determines which facade families have consumers.
// Keep these exports available without duplicating the profile gates here.
#[allow(unused_imports)]
pub use attrs::*;
pub use core::*;
#[allow(unused_imports)]
pub use ext_state::*;
#[allow(unused_imports)]
pub use iteration::*;
#[allow(unused_imports)]
pub use itertools::*;
pub use path::*;
